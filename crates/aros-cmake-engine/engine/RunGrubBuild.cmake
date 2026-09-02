cmake_minimum_required(VERSION 3.22)

include("${CMAKE_CURRENT_LIST_DIR}/GrubSourceLock.cmake")
set(_GB_ARCHIVE_SHA256 "${_AROS_GRUB2_ARCHIVE_SHA256}")
set(_GB_SOURCE_URL "${_AROS_GRUB2_SOURCE_URL}")
set(_GB_PATCH_RELATIVE "arch/all-pc/boot/grub2-aros/grub-2.12-aros.diff")

function(_gb_real_path path output)
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

function(_gb_reject_symlink_components root path label)
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

function(_gb_require_regular_file path label)
    if(NOT EXISTS "${path}" OR IS_DIRECTORY "${path}" OR IS_SYMLINK "${path}")
        message(FATAL_ERROR "${label} must be an existing regular file")
    endif()
endfunction()

function(_gb_require_executable path label)
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

function(_gb_run_in directory description)
    execute_process(
        COMMAND ${ARGN}
        WORKING_DIRECTORY "${directory}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${description} failed (${_result})\n${_stdout}${_stderr}")
    endif()
endfunction()

function(_gb_file_matches label path)
    execute_process(
        COMMAND "${GB_FILE}" -b "${path}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _description
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "cannot inspect ${label}: ${_stderr}")
    endif()
    foreach(_pattern IN LISTS ARGN)
        if(NOT _description MATCHES "${_pattern}")
            message(FATAL_ERROR
                "${label} has the wrong format: ${_description} (missing ${_pattern})")
        endif()
    endforeach()
endfunction()

function(_gb_manifest_sha256 root pattern output_count output_sha256)
    file(GLOB_RECURSE _entries RELATIVE "${root}" LIST_DIRECTORIES false "${root}/${pattern}")
    set(_files "")
    foreach(_relative IN LISTS _entries)
        set(_path "${root}/${_relative}")
        if(IS_DIRECTORY "${_path}" OR IS_SYMLINK "${_path}")
            message(FATAL_ERROR "installed GRUB2 product is not a regular file: ${_relative}")
        endif()
        list(APPEND _files "${_relative}")
    endforeach()
    list(SORT _files)
    list(LENGTH _files _count)
    list(JOIN _files "\n" _manifest)
    if(_files)
        string(APPEND _manifest "\n")
    endif()
    string(SHA256 _sha256 "${_manifest}")
    set(${output_count} "${_count}" PARENT_SCOPE)
    set(${output_sha256} "${_sha256}" PARENT_SCOPE)
endfunction()

function(_gb_lane_contract mode)
    if(mode STREQUAL "pc")
        set(_id "grub2-host")
        set(_lane "pc")
        set(_configure_target "x86_64")
        set(_platform "pc")
        set(_triple "i386-pc-linux-gnu")
        set(_isa_flags "--target=i386-pc-linux-gnu -march=i486 -m32")
        set(_link_format "-melf_i386")
        set(_platform_dir "i386-pc")
        set(_file_count 615)
        set(_manifest_relative "cmake/manifests/grub-2.12-pc.install")
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
        set(_id "grub2-efi-host")
        set(_lane "efi-x86_64")
        set(_configure_target "x86_64")
        set(_platform "efi")
        set(_triple "x86_64-pc-linux-gnu")
        set(_isa_flags "--target=x86_64-pc-linux-gnu")
        set(_link_format "-melf_x86_64")
        set(_platform_dir "x86_64-efi")
        set(_file_count 591)
        set(_manifest_relative "cmake/manifests/grub-2.12-efi64.install")
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
        set(_id "grub2-efi32-host")
        set(_lane "efi-i386")
        set(_configure_target "i386")
        set(_platform "efi")
        set(_triple "i386-pc-linux-gnu")
        set(_isa_flags "--target=i386-pc-linux-gnu -march=i486 -m32")
        set(_link_format "-melf_i386")
        set(_platform_dir "i386-efi")
        set(_file_count 593)
        set(_manifest_relative "cmake/manifests/grub-2.12-efi32.install")
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
        message(FATAL_ERROR "GRUB2 runner received unsupported mode ${mode}")
    endif()
    set(GB_EXPECTED_id "${_id}" PARENT_SCOPE)
    set(GB_EXPECTED_lane "${_lane}" PARENT_SCOPE)
    set(GB_EXPECTED_configure_target "${_configure_target}" PARENT_SCOPE)
    set(GB_EXPECTED_platform "${_platform}" PARENT_SCOPE)
    set(GB_EXPECTED_triple "${_triple}" PARENT_SCOPE)
    set(GB_EXPECTED_isa_flags "${_isa_flags}" PARENT_SCOPE)
    set(GB_EXPECTED_link_format "${_link_format}" PARENT_SCOPE)
    set(GB_EXPECTED_platform_dir "${_platform_dir}" PARENT_SCOPE)
    set(GB_EXPECTED_file_count "${_file_count}" PARENT_SCOPE)
    set(GB_EXPECTED_manifest_relative "${_manifest_relative}" PARENT_SCOPE)
    set(GB_EXPECTED_private_relative "${_private_relative}" PARENT_SCOPE)
endfunction()

if(NOT DEFINED GB_ACTION OR NOT GB_ACTION MATCHES "^(fetch|build)$")
    message(FATAL_ERROR "RunGrubBuild requires GB_ACTION=fetch or GB_ACTION=build")
endif()
if(NOT DEFINED CONTRACT OR NOT EXISTS "${CONTRACT}" OR IS_DIRECTORY "${CONTRACT}" OR
   IS_SYMLINK "${CONTRACT}")
    message(FATAL_ERROR "RunGrubBuild requires a regular existing CONTRACT")
endif()
execute_process(
    COMMAND /usr/bin/uname -s
    RESULT_VARIABLE _host_system_result
    OUTPUT_VARIABLE _host_system
    ERROR_VARIABLE _host_system_error)
execute_process(
    COMMAND /usr/bin/uname -m
    RESULT_VARIABLE _host_machine_result
    OUTPUT_VARIABLE _host_machine
    ERROR_VARIABLE _host_machine_error)
string(STRIP "${_host_system}" _host_system)
string(STRIP "${_host_machine}" _host_machine)
if(NOT _host_system_result EQUAL 0 OR NOT _host_machine_result EQUAL 0 OR
   NOT "${_host_system}" STREQUAL "Darwin" OR
   NOT "${_host_machine}" MATCHES "^(arm64|aarch64)$")
    message(FATAL_ERROR
        "GRUB2 runner requires the audited arm64 Darwin host "
        "(${_host_system}/${_host_machine}; ${_host_system_error}${_host_machine_error})")
endif()
include("${CONTRACT}")

if(GB_ACTION STREQUAL "fetch")
    foreach(_required IN ITEMS GB_BUILD_ROOT GB_ARCHIVE GB_SOURCE_URL GB_ARCHIVE_SHA256)
        if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
            message(FATAL_ERROR "GRUB2 fetch contract omits ${_required}")
        endif()
    endforeach()
    if(NOT GB_SOURCE_URL STREQUAL _GB_SOURCE_URL OR
       NOT GB_ARCHIVE_SHA256 STREQUAL _GB_ARCHIVE_SHA256)
        message(FATAL_ERROR "GRUB2 fetch contract differs from the audited source identity")
    endif()
    if(NOT EXISTS "${GB_BUILD_ROOT}" OR NOT IS_DIRECTORY "${GB_BUILD_ROOT}")
        message(FATAL_ERROR "GRUB2 fetch build root is unavailable")
    endif()
    _gb_real_path("${GB_BUILD_ROOT}" _build_root)
    _gb_real_path("${GB_ARCHIVE}" _archive)
    set(_expected_archive "${_build_root}/downloads/grub-2.12.tar.xz")
    _gb_real_path("${_expected_archive}" _expected_archive)
    cmake_path(IS_PREFIX _build_root "${_archive}" NORMALIZE _archive_owned)
    if(NOT _archive_owned OR NOT _archive STREQUAL _expected_archive)
        message(FATAL_ERROR "GRUB2 fetch archive escapes its private cache")
    endif()
    _gb_reject_symlink_components("${_build_root}" "${_archive}" "GRUB2 archive path")
    if(EXISTS "${_archive}")
        _gb_require_regular_file("${_archive}" "GRUB2 archive")
        file(SHA256 "${_archive}" _actual_sha256)
        if(_actual_sha256 STREQUAL _GB_ARCHIVE_SHA256)
            return()
        endif()
        file(REMOVE "${_archive}")
    endif()
    cmake_path(GET _archive PARENT_PATH _archive_parent)
    file(MAKE_DIRECTORY "${_archive_parent}")
    _gb_real_path("${_archive_parent}" _archive_parent_real)
    cmake_path(IS_PREFIX _build_root "${_archive_parent_real}" NORMALIZE _parent_owned)
    if(NOT _parent_owned OR IS_SYMLINK "${_archive_parent}")
        message(FATAL_ERROR "GRUB2 archive parent escaped its private cache")
    endif()
    set(_partial "${_archive}.part")
    if(IS_SYMLINK "${_partial}")
        message(FATAL_ERROR "GRUB2 archive partial path is a symlink")
    endif()
    if(EXISTS "${_partial}")
        file(REMOVE "${_partial}")
    endif()
    file(DOWNLOAD "${_GB_SOURCE_URL}" "${_partial}"
        EXPECTED_HASH "SHA256=${_GB_ARCHIVE_SHA256}"
        TLS_VERIFY ON
        STATUS _status
        LOG _log)
    list(GET _status 0 _status_code)
    if(NOT _status_code EQUAL 0)
        file(REMOVE "${_partial}")
        message(FATAL_ERROR "downloading audited GRUB2 source failed: ${_status}; ${_log}")
    endif()
    _gb_require_regular_file("${_partial}" "downloaded GRUB2 archive")
    file(RENAME "${_partial}" "${_archive}")
    _gb_require_regular_file("${_archive}" "downloaded GRUB2 archive")
    file(SHA256 "${_archive}" _actual_sha256)
    if(NOT _actual_sha256 STREQUAL _GB_ARCHIVE_SHA256)
        message(FATAL_ERROR "downloaded GRUB2 archive differs from audited SHA-256")
    endif()
    return()
endif()

set(_required GB_MODE GB_MMAKE_ID GB_SOURCE_ROOT GB_BUILD_ROOT GB_BINARY_DIR
    GB_INSTALL_PREFIX GB_ARCHIVE GB_PATCH GB_INSTALL_MANIFEST GB_SOURCE_URL
    GB_ARCHIVE_SHA256 GB_PATCH_SHA256 GB_XZ_PREFIX GB_HOST_PATH GB_HOST_CC
    GB_HOST_CXX GB_PATCH_TOOL GB_MAKE GB_FILE GB_OTOOL GB_INSTALL_TOOL
    GB_MKDIR_TOOL GB_AWK GB_PKG_CONFIG GB_YACC GB_LEX GB_MAKEINFO GB_PYTHON
    GB_SED GB_MSGFMT GB_MSGMERGE GB_XGETTEXT GB_AR GB_RANLIB GB_NM GB_CMP
    GB_GREP GB_TARGET_CLANG GB_TARGET_LD GB_TARGET_OBJCOPY GB_TARGET_RANLIB GB_TARGET_NM
    GB_TARGET_STRIP GB_CONFIGURE_TARGET GB_PLATFORM GB_TARGET_TRIPLE
    GB_TARGET_ISA_FLAGS GB_LINK_FORMAT GB_PLATFORM_DIR GB_EXPECTED_FILE_COUNT
    GB_INSTALL_MANIFEST_SHA256 GB_STAMP)
foreach(_required IN LISTS _required)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "GRUB2 build contract omits ${_required}")
    endif()
endforeach()
if(NOT DEFINED GB_PRIVATE_PRODUCTS OR NOT DEFINED GB_INSTALL_PRODUCTS OR
   NOT GB_PRIVATE_PRODUCTS OR NOT GB_INSTALL_PRODUCTS)
    message(FATAL_ERROR "GRUB2 build contract omits its product lists")
endif()
_gb_lane_contract("${GB_MODE}")
if(NOT "${GB_MMAKE_ID}" STREQUAL "${GB_EXPECTED_id}" OR
   NOT "${GB_CONFIGURE_TARGET}" STREQUAL "${GB_EXPECTED_configure_target}" OR
   NOT "${GB_PLATFORM}" STREQUAL "${GB_EXPECTED_platform}" OR
   NOT "${GB_TARGET_TRIPLE}" STREQUAL "${GB_EXPECTED_triple}" OR
   NOT "${GB_TARGET_ISA_FLAGS}" STREQUAL "${GB_EXPECTED_isa_flags}" OR
   NOT "${GB_LINK_FORMAT}" STREQUAL "${GB_EXPECTED_link_format}" OR
   NOT "${GB_PLATFORM_DIR}" STREQUAL "${GB_EXPECTED_platform_dir}" OR
   NOT "${GB_EXPECTED_FILE_COUNT}" EQUAL "${GB_EXPECTED_file_count}")
    message(FATAL_ERROR
        "GRUB2 build contract differs from its audited lane identity\n"
        "id=${GB_MMAKE_ID}/${GB_EXPECTED_id}; "
        "target=${GB_CONFIGURE_TARGET}/${GB_EXPECTED_configure_target}; "
        "platform=${GB_PLATFORM}/${GB_EXPECTED_platform}; "
        "triple=${GB_TARGET_TRIPLE}/${GB_EXPECTED_triple}; "
        "isa=${GB_TARGET_ISA_FLAGS}/${GB_EXPECTED_isa_flags}; "
        "link=${GB_LINK_FORMAT}/${GB_EXPECTED_link_format}; "
        "directory=${GB_PLATFORM_DIR}/${GB_EXPECTED_platform_dir}; "
        "count=${GB_EXPECTED_FILE_COUNT}/${GB_EXPECTED_file_count}")
endif()
if(NOT GB_SOURCE_URL STREQUAL _GB_SOURCE_URL OR
   NOT GB_ARCHIVE_SHA256 STREQUAL _GB_ARCHIVE_SHA256)
    message(FATAL_ERROR "GRUB2 build contract differs from audited source identity")
endif()

foreach(_directory IN ITEMS SOURCE_ROOT BUILD_ROOT)
    if(NOT EXISTS "${GB_${_directory}}" OR NOT IS_DIRECTORY "${GB_${_directory}}")
        message(FATAL_ERROR "GRUB2 build contract has no directory ${_directory}")
    endif()
endforeach()
foreach(_path IN ITEMS PATCH ARCHIVE INSTALL_MANIFEST)
    _gb_require_regular_file("${GB_${_path}}" "GRUB2 ${_path}")
endforeach()
_gb_real_path("${GB_SOURCE_ROOT}" _source_root)
_gb_real_path("${GB_BUILD_ROOT}" _build_root)
_gb_real_path("${GB_BINARY_DIR}" _binary_dir)
_gb_real_path("${GB_INSTALL_PREFIX}" _install_prefix)
_gb_real_path("${GB_PATCH}" _patch)
_gb_real_path("${GB_ARCHIVE}" _archive)
_gb_real_path("${GB_INSTALL_MANIFEST}" _install_manifest)
_gb_real_path("${GB_STAMP}" _stamp)
if(IS_SYMLINK "${_source_root}" OR IS_SYMLINK "${_build_root}")
    message(FATAL_ERROR "GRUB2 runner source or build root is a symlink")
endif()
set(_expected_patch "${_source_root}/${_GB_PATCH_RELATIVE}")
_gb_real_path("${_expected_patch}" _expected_patch)
set(_expected_archive "${_build_root}/downloads/grub-2.12.tar.xz")
_gb_real_path("${_expected_archive}" _expected_archive)
set(_expected_install_manifest
    "${_source_root}/${GB_EXPECTED_manifest_relative}")
_gb_real_path("${_expected_install_manifest}" _expected_install_manifest)
set(_configure_root "${_build_root}/gen/configure")
_gb_real_path("${_configure_root}" _configure_root)
set(_expected_binary
    "${_build_root}/gen/configure/arch/all-pc/boot/grub2-host/${GB_EXPECTED_lane}")
set(_expected_prefix "${_build_root}/hosttools/grub2/${GB_EXPECTED_lane}")
_gb_real_path("${_expected_binary}" _expected_binary)
_gb_real_path("${_expected_prefix}" _expected_prefix)
cmake_path(IS_PREFIX _build_root "${_configure_root}" NORMALIZE _configure_owned)
cmake_path(IS_PREFIX _configure_root "${_binary_dir}" NORMALIZE _binary_owned)
cmake_path(IS_PREFIX _build_root "${_install_prefix}" NORMALIZE _prefix_owned)
cmake_path(IS_PREFIX _build_root "${_archive}" NORMALIZE _archive_owned)
cmake_path(IS_PREFIX _source_root "${_patch}" NORMALIZE _patch_owned)
cmake_path(IS_PREFIX _source_root "${_install_manifest}" NORMALIZE
    _install_manifest_owned)
if(NOT _configure_owned OR _configure_root STREQUAL _build_root OR
   NOT _binary_owned OR _binary_dir STREQUAL _configure_root OR
   NOT _prefix_owned OR _install_prefix STREQUAL _build_root OR
   NOT _archive_owned OR NOT _patch_owned OR NOT _install_manifest_owned OR
   NOT _binary_dir STREQUAL _expected_binary OR
   NOT _install_prefix STREQUAL _expected_prefix OR
   NOT _archive STREQUAL _expected_archive OR
   NOT _patch STREQUAL _expected_patch OR
   NOT _install_manifest STREQUAL _expected_install_manifest)
    message(FATAL_ERROR "GRUB2 runner contract has escaped or substituted paths")
endif()
_gb_reject_symlink_components("${_build_root}" "${_configure_root}" "GRUB2 configure root")
_gb_reject_symlink_components("${_configure_root}" "${_binary_dir}" "GRUB2 binary directory")
_gb_reject_symlink_components("${_build_root}" "${_install_prefix}" "GRUB2 install prefix")
_gb_reject_symlink_components("${_build_root}" "${_archive}" "GRUB2 archive path")
cmake_path(IS_PREFIX _binary_dir "${_install_prefix}" NORMALIZE _binary_contains_prefix)
cmake_path(IS_PREFIX _install_prefix "${_binary_dir}" NORMALIZE _prefix_contains_binary)
if(_binary_contains_prefix OR _prefix_contains_binary)
    message(FATAL_ERROR "GRUB2 runner binary directory overlaps install prefix")
endif()

file(SHA256 "${_patch}" _actual_patch_sha256)
file(SHA256 "${_archive}" _actual_archive_sha256)
file(SHA256 "${_install_manifest}" _actual_install_manifest_sha256)
if(NOT _actual_patch_sha256 STREQUAL GB_PATCH_SHA256 OR
   NOT _actual_archive_sha256 STREQUAL _GB_ARCHIVE_SHA256 OR
   NOT _actual_install_manifest_sha256 STREQUAL GB_INSTALL_MANIFEST_SHA256)
    message(FATAL_ERROR "GRUB2 runner inputs changed after configuration; rerun CMake")
endif()

if(NOT EXISTS "${GB_XZ_PREFIX}" OR NOT IS_DIRECTORY "${GB_XZ_PREFIX}")
    message(FATAL_ERROR "GRUB2 runner Homebrew xz dependency is unavailable")
endif()
_gb_real_path("${GB_XZ_PREFIX}" _xz_prefix)
_gb_real_path("/opt/homebrew/opt/xz" _expected_xz_prefix)
if(NOT _xz_prefix STREQUAL _expected_xz_prefix OR
   NOT EXISTS "${_xz_prefix}/include/lzma.h" OR
   NOT EXISTS "${_xz_prefix}/lib/liblzma.dylib" OR
   NOT EXISTS "${_xz_prefix}/lib/pkgconfig/liblzma.pc")
    message(FATAL_ERROR "GRUB2 runner Homebrew xz dependency is incomplete or substituted")
endif()
set(_expected_host_path
    "/opt/homebrew/opt/gettext/bin:/opt/homebrew/opt/texinfo/bin:/opt/homebrew/opt/gawk/bin:/opt/homebrew/opt/pkgconf/bin:/opt/homebrew/opt/python@3.14/bin:/opt/homebrew/opt/gnu-sed/bin:/opt/homebrew/opt/coreutils/bin:/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:/usr/bin:/bin:/usr/sbin:/sbin")
if(NOT GB_HOST_PATH STREQUAL _expected_host_path)
    message(FATAL_ERROR "GRUB2 runner host PATH contract was substituted")
endif()
string(REPLACE ":" ";" _host_path_entries "${GB_HOST_PATH}")
foreach(_path_entry IN LISTS _host_path_entries)
    if(NOT EXISTS "${_path_entry}" OR NOT IS_DIRECTORY "${_path_entry}")
        message(FATAL_ERROR "GRUB2 runner host PATH entry is unavailable: ${_path_entry}")
    endif()
endforeach()

set(_host_tool_contract
    "GB_HOST_CC|/usr/bin/cc"
    "GB_HOST_CXX|/usr/bin/c++"
    "GB_PATCH_TOOL|/usr/bin/patch"
    "GB_MAKE|/usr/bin/make"
    "GB_FILE|/usr/bin/file"
    "GB_OTOOL|/usr/bin/otool"
    "GB_INSTALL_TOOL|/opt/homebrew/opt/coreutils/bin/ginstall"
    "GB_MKDIR_TOOL|/opt/homebrew/opt/coreutils/bin/gmkdir"
    "GB_AWK|/opt/homebrew/opt/gawk/bin/gawk"
    "GB_PKG_CONFIG|/opt/homebrew/opt/pkgconf/bin/pkg-config"
    "GB_YACC|/usr/bin/bison"
    "GB_LEX|/usr/bin/flex"
    "GB_MAKEINFO|/opt/homebrew/opt/texinfo/bin/makeinfo"
    "GB_PYTHON|/opt/homebrew/opt/python@3.14/bin/python3"
    "GB_SED|/opt/homebrew/opt/gnu-sed/bin/gsed"
    "GB_MSGFMT|/opt/homebrew/opt/gettext/bin/msgfmt"
    "GB_MSGMERGE|/opt/homebrew/opt/gettext/bin/msgmerge"
    "GB_XGETTEXT|/opt/homebrew/opt/gettext/bin/xgettext"
    "GB_AR|/usr/bin/ar"
    "GB_RANLIB|/usr/bin/ranlib"
    "GB_NM|/usr/bin/nm"
    "GB_CMP|/usr/bin/cmp"
    "GB_GREP|/usr/bin/grep"
    "GB_TARGET_CLANG|/opt/homebrew/opt/llvm/bin/clang"
    "GB_TARGET_LD|/opt/homebrew/opt/lld/bin/ld.lld"
    "GB_TARGET_OBJCOPY|/opt/homebrew/opt/llvm/bin/llvm-objcopy"
    "GB_TARGET_RANLIB|/opt/homebrew/opt/llvm/bin/llvm-ranlib"
    "GB_TARGET_NM|/opt/homebrew/opt/llvm/bin/llvm-nm"
    "GB_TARGET_STRIP|/opt/homebrew/opt/llvm/bin/llvm-strip")
foreach(_tool_pair IN LISTS _host_tool_contract)
    string(REPLACE "|" ";" _tool_parts "${_tool_pair}")
    list(GET _tool_parts 0 _contract_variable)
    list(GET _tool_parts 1 _expected_tool)
    if(NOT "${${_contract_variable}}" STREQUAL "${_expected_tool}")
        message(FATAL_ERROR
            "GRUB2 runner host tool ${_contract_variable} was substituted")
    endif()
    _gb_require_executable("${${_contract_variable}}"
        "GRUB2 runner host tool ${_contract_variable}")
    _gb_real_path("${${_contract_variable}}" _actual_tool_real)
    _gb_real_path("${_expected_tool}" _expected_tool_real)
    if(NOT _actual_tool_real STREQUAL _expected_tool_real)
        message(FATAL_ERROR
            "GRUB2 runner host tool ${_contract_variable} escaped its audited path")
    endif()
endforeach()

set(_expected_private_products "")
foreach(_relative IN LISTS GB_EXPECTED_private_relative)
    list(APPEND _expected_private_products "${_binary_dir}/${_relative}")
endforeach()
file(STRINGS "${_install_manifest}" _install_relative)
list(LENGTH _install_relative _install_manifest_count)
set(_sorted_install_relative "${_install_relative}")
list(SORT _sorted_install_relative)
if(NOT _install_manifest_count EQUAL GB_EXPECTED_file_count OR
   NOT "${_install_relative}" STREQUAL "${_sorted_install_relative}")
    message(FATAL_ERROR "GRUB2 runner install manifest count or ordering is invalid")
endif()
set(_expected_install_products "")
foreach(_relative IN LISTS _install_relative)
    list(APPEND _expected_install_products "${_install_prefix}/${_relative}")
endforeach()
if(NOT GB_PRIVATE_PRODUCTS STREQUAL _expected_private_products OR
   NOT GB_INSTALL_PRODUCTS STREQUAL _expected_install_products OR
   NOT _stamp STREQUAL "${_binary_dir}/.grub2-${GB_EXPECTED_lane}.stamp")
    message(FATAL_ERROR "GRUB2 runner product contract differs from audited lane")
endif()
foreach(_product IN LISTS GB_PRIVATE_PRODUCTS GB_INSTALL_PRODUCTS)
    _gb_real_path("${_product}" _product_real)
    cmake_path(IS_PREFIX _binary_dir "${_product_real}" NORMALIZE _private_product)
    cmake_path(IS_PREFIX _install_prefix "${_product_real}" NORMALIZE _install_product)
    if(NOT _private_product AND NOT _install_product)
        message(FATAL_ERROR "GRUB2 runner product escaped its private owner")
    endif()
endforeach()

# Both trees are exact private lane roots.  A missing declared product therefore
# repairs atomically without trusting stale make dependencies, and extraction
# plus patching is always performed below BINARY_DIR rather than the checkout.
file(REMOVE_RECURSE "${_binary_dir}" "${_install_prefix}")
file(MAKE_DIRECTORY "${_binary_dir}" "${_install_prefix}")
_gb_reject_symlink_components("${_configure_root}" "${_binary_dir}" "GRUB2 binary directory")
_gb_reject_symlink_components("${_build_root}" "${_install_prefix}" "GRUB2 install prefix")
set(_stage_root "${_binary_dir}/source")
file(MAKE_DIRECTORY "${_stage_root}")
file(ARCHIVE_EXTRACT INPUT "${_archive}" DESTINATION "${_stage_root}")
set(_source_stage "${_stage_root}/grub-2.12")
if(NOT EXISTS "${_source_stage}" OR NOT IS_DIRECTORY "${_source_stage}" OR
   IS_SYMLINK "${_source_stage}")
    message(FATAL_ERROR "GRUB2 archive did not extract its audited source root")
endif()
_gb_run_in("${_binary_dir}" "patching staged GRUB2 source"
    "${GB_PATCH_TOOL}" -d "${_source_stage}" -p1 --batch --forward -i "${_patch}")
set(_build_dir "${_binary_dir}/build")
file(MAKE_DIRECTORY "${_build_dir}")

set(_target_link_flags
    "${GB_TARGET_ISA_FLAGS} -fuse-ld=lld -Wl,--image-base=0 -nostartfiles")
set(_build_environment
    "PATH=${GB_HOST_PATH}"
    "CONFIG_SHELL=/bin/sh"
    "SHELL=/bin/sh"
    "LC_ALL=C"
    "LANG=C"
    "MAKEFLAGS="
    "MFLAGS="
    "CONFIG_SITE="
    "CDPATH="
    "ENV="
    "BASH_ENV="
    "CC=${GB_HOST_CC}"
    "CXX=${GB_HOST_CXX}"
    "CPP=${GB_HOST_CC} -E"
    "CFLAGS="
    "CXXFLAGS="
    "OBJCFLAGS="
    "LIBS="
    "CPATH="
    "C_INCLUDE_PATH="
    "CPLUS_INCLUDE_PATH="
    "LIBRARY_PATH="
    "AR=${GB_AR}"
    "RANLIB=${GB_RANLIB}"
    "NM=${GB_NM}"
    "AWK=${GB_AWK}"
    "CMP=${GB_CMP}"
    "INSTALL=${GB_INSTALL_TOOL} -c"
    "MKDIR_P=${GB_MKDIR_TOOL} -p"
    "PKG_CONFIG=${GB_PKG_CONFIG}"
    "YACC=${GB_YACC}"
    "LEX=${GB_LEX}"
    "MAKEINFO=${GB_MAKEINFO}"
    "PYTHON=${GB_PYTHON}"
    "PYTHONNOUSERSITE=1"
    "PYTHONHASHSEED=0"
    "SED=${GB_SED}"
    "GREP=${GB_GREP}"
    "EGREP=${GB_GREP} -E"
    "MSGFMT=${GB_MSGFMT}"
    "GMSGFMT=${GB_MSGFMT}"
    "MSGMERGE=${GB_MSGMERGE}"
    "XGETTEXT=${GB_XGETTEXT}"
    "CPPFLAGS=-I${GB_XZ_PREFIX}/include"
    "LDFLAGS=-L${GB_XZ_PREFIX}/lib"
    "PKG_CONFIG_PATH="
    "PKG_CONFIG_LIBDIR=${GB_XZ_PREFIX}/lib/pkgconfig"
    "PKG_CONFIG_SYSROOT_DIR="
    "TARGET_CC=${GB_TARGET_CLANG}"
    "TARGET_CPP=${GB_TARGET_CLANG} -E"
    "TARGET_CCAS=${GB_TARGET_CLANG}"
    "TARGET_LD=${GB_TARGET_LD}"
    "TARGET_OBJCOPY=${GB_TARGET_OBJCOPY}"
    "TARGET_RANLIB=${GB_TARGET_RANLIB}"
    "TARGET_NM=${GB_TARGET_NM}"
    "TARGET_STRIP=${GB_TARGET_STRIP}"
    "TARGET_CPPFLAGS=${GB_TARGET_ISA_FLAGS}"
    "TARGET_CFLAGS=${GB_TARGET_ISA_FLAGS}"
    "TARGET_CCASFLAGS=${GB_TARGET_ISA_FLAGS}"
    "TARGET_LDFLAGS=${_target_link_flags}"
    "grub_cv_target_cc_link_format=${GB_LINK_FORMAT}")
set(_configure_args
    "--build=arm64-apple-darwin"
    "--host=arm64-apple-darwin"
    "--target=${GB_CONFIGURE_TARGET}"
    "--with-platform=${GB_PLATFORM}"
    "--prefix=${_install_prefix}"
    "--datarootdir=${_install_prefix}/share"
    "--sysconfdir=${_install_prefix}/etc"
    "--bindir=${_install_prefix}"
    "--sbindir=${_install_prefix}"
    "--libdir=${_install_prefix}/lib"
    "--enable-silent-rules"
    "--disable-grub-mkfont"
    "--disable-werror"
    "--program-prefix="
    "--enable-liblzma")
_gb_run_in("${_build_dir}" "configuring GRUB2 ${GB_MODE}"
    "${CMAKE_COMMAND}" -E env ${_build_environment}
    "${_source_stage}/configure" ${_configure_args})
_gb_run_in("${_build_dir}" "building GRUB2 ${GB_MODE}"
    "${CMAKE_COMMAND}" -E env ${_build_environment}
    "${GB_MAKE}" -j4)
_gb_run_in("${_build_dir}" "installing GRUB2 ${GB_MODE}"
    "${CMAKE_COMMAND}" -E env ${_build_environment}
    "${GB_MAKE}" install)

foreach(_product IN LISTS GB_PRIVATE_PRODUCTS GB_INSTALL_PRODUCTS)
    _gb_require_regular_file("${_product}" "declared GRUB2 product")
endforeach()
_gb_file_matches("native grub-mkimage" "${_build_dir}/grub-mkimage" "Mach-O" "arm64")
if(GB_MODE STREQUAL "pc")
    _gb_file_matches("PC boot.img" "${_build_dir}/grub-core/boot.img" "DOS/MBR boot sector")
    _gb_file_matches("PC kernel.img" "${_build_dir}/grub-core/kernel.img" "ELF 32-bit" "Intel 80386")
    _gb_file_matches("PC normal.mod" "${_build_dir}/grub-core/normal.mod" "ELF 32-bit" "Intel 80386" "relocatable")
else()
    if(GB_MODE STREQUAL "efi64")
        set(_elf_bits "ELF 64-bit")
        set(_elf_arch "x86-64")
    else()
        set(_elf_bits "ELF 32-bit")
        set(_elf_arch "Intel 80386")
    endif()
    _gb_file_matches("EFI kernel.img" "${_build_dir}/grub-core/kernel.img"
        "${_elf_bits}" "${_elf_arch}" "relocatable")
    _gb_file_matches("EFI normal.mod" "${_build_dir}/grub-core/normal.mod"
        "${_elf_bits}" "${_elf_arch}" "relocatable")
endif()
execute_process(
    COMMAND "${GB_OTOOL}" -L "${_build_dir}/grub-mkimage"
    RESULT_VARIABLE _otool_result
    OUTPUT_VARIABLE _otool_output
    ERROR_VARIABLE _otool_error)
if(NOT _otool_result EQUAL 0 OR
   NOT _otool_output MATCHES "/opt/homebrew/opt/xz/lib/liblzma")
    message(FATAL_ERROR
        "native grub-mkimage did not link the audited Homebrew liblzma\n${_otool_output}${_otool_error}")
endif()

_gb_manifest_sha256("${_install_prefix}" "*" _installed_count _installed_sha256)
if(NOT _installed_count EQUAL GB_EXPECTED_FILE_COUNT OR
   NOT _installed_sha256 STREQUAL GB_INSTALL_MANIFEST_SHA256)
    message(FATAL_ERROR
        "GRUB2 ${GB_MODE} install manifest differs from the audited product set "
        "(count ${_installed_count}, sha256 ${_installed_sha256})")
endif()
file(TOUCH "${_stamp}")
