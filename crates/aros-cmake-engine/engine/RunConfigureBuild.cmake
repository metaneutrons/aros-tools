cmake_minimum_required(VERSION 3.22)

# Match ConfigureBuild.cmake's physical-path handling for non-existing output
# tails.  In particular, /tmp may spell a symlink to /private/tmp on macOS.
function(_cb_real_path path output)
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

if(NOT DEFINED CONTRACT OR NOT EXISTS "${CONTRACT}")
    message(FATAL_ERROR "RunConfigureBuild requires an existing CONTRACT")
endif()
include("${CONTRACT}")

if(NOT CB_MODE MATCHES "^(adflib-host|adflib-target|wirelessmanager)$")
    message(FATAL_ERROR "configure runner received unsupported mode ${CB_MODE}")
endif()
set(_required CB_MODE CB_SOURCE_ROOT CB_BUILD_ROOT CB_SOURCE_DIR CB_BINARY_DIR
    CB_INSTALL_PREFIX CB_INPUT_MANIFEST CB_INPUT_MANIFEST_SHA256 CB_COMPILER
    CB_ARCHIVER CB_RANLIB CB_MAKE CB_SHELL CB_INPUT_RELATIVE CB_INPUT_SHA256)
if(CB_MODE STREQUAL "wirelessmanager")
    list(APPEND _required CB_LINKER)
endif()
foreach(_required IN LISTS _required)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "configure runner contract omits ${_required}")
    endif()
endforeach()

# Re-resolve all path components before removing or creating anything.  The
# generated contract already contains physical paths, but this second check
# makes a stale or hand-written contract fail closed if a directory becomes a
# symlink between configure and build time.
foreach(_required IN ITEMS SOURCE_ROOT BUILD_ROOT SOURCE_DIR)
    if(NOT EXISTS "${CB_${_required}}" OR NOT IS_DIRECTORY "${CB_${_required}}")
        message(FATAL_ERROR "configure runner contract has no directory ${_required}")
    endif()
endforeach()
if(NOT EXISTS "${CB_INPUT_MANIFEST}" OR IS_DIRECTORY "${CB_INPUT_MANIFEST}" OR
   IS_SYMLINK "${CB_INPUT_MANIFEST}")
    message(FATAL_ERROR "configure runner contract has no regular input manifest")
endif()
_cb_real_path("${CB_SOURCE_ROOT}" _source_root)
_cb_real_path("${CB_BUILD_ROOT}" _build_root)
_cb_real_path("${CB_SOURCE_DIR}" _source_dir)
_cb_real_path("${CB_BINARY_DIR}" _binary_dir)
_cb_real_path("${CB_INSTALL_PREFIX}" _install_prefix)
_cb_real_path("${CB_INPUT_MANIFEST}" _input_manifest)
cmake_path(IS_PREFIX _source_root "${_source_dir}" NORMALIZE _source_owned)
cmake_path(IS_PREFIX _source_dir "${_input_manifest}" NORMALIZE _manifest_owned)
if(NOT _source_owned OR _source_dir STREQUAL _source_root OR
   NOT _manifest_owned OR _input_manifest STREQUAL _source_dir)
    message(FATAL_ERROR "configure runner contract has escaped source paths")
endif()
set(_configure_root "${_build_root}/gen/configure")
_cb_real_path("${_configure_root}" _configure_root)
cmake_path(IS_PREFIX _build_root "${_configure_root}" NORMALIZE _configure_root_owned)
cmake_path(IS_PREFIX _configure_root "${_binary_dir}" NORMALIZE _binary_owned)
cmake_path(IS_PREFIX _build_root "${_install_prefix}" NORMALIZE _prefix_owned)
if(NOT _configure_root_owned OR _configure_root STREQUAL _build_root OR
   NOT _binary_owned OR _binary_dir STREQUAL _configure_root OR
   NOT _prefix_owned OR _install_prefix STREQUAL _build_root)
    message(FATAL_ERROR "configure runner contract has escaped build paths")
endif()
foreach(_owned IN ITEMS source_dir input_manifest install_prefix)
    set(_owner_path "${_${_owned}}")
    cmake_path(IS_PREFIX _binary_dir "${_owner_path}" NORMALIZE _binary_contains)
    cmake_path(IS_PREFIX _owner_path "${_binary_dir}" NORMALIZE _owner_contains)
    if(_binary_contains OR _owner_contains)
        message(FATAL_ERROR "configure runner contract overlaps binary and ${_owned}")
    endif()
endforeach()
set(_private_products "")
foreach(_product IN LISTS CB_PRIVATE_PRODUCTS)
    _cb_real_path("${_product}" _product_real)
    cmake_path(IS_PREFIX _binary_dir "${_product_real}" NORMALIZE _product_owned)
    if(NOT _product_owned OR _product_real STREQUAL _binary_dir)
        message(FATAL_ERROR "configure runner contract has escaped private product")
    endif()
    list(APPEND _private_products "${_product_real}")
endforeach()
set(_install_products "")
foreach(_product IN LISTS CB_INSTALL_PRODUCTS)
    _cb_real_path("${_product}" _product_real)
    cmake_path(IS_PREFIX _install_prefix "${_product_real}" NORMALIZE _product_owned)
    if(NOT _product_owned OR _product_real STREQUAL _install_prefix)
        message(FATAL_ERROR "configure runner contract has escaped installed product")
    endif()
    list(APPEND _install_products "${_product_real}")
endforeach()

function(_cb_run description)
    execute_process(
        COMMAND ${ARGN}
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "${description} failed (${_result})\n${_stdout}${_stderr}")
    endif()
endfunction()

file(SHA256 "${_input_manifest}" _actual_manifest_sha256)
if(NOT _actual_manifest_sha256 STREQUAL CB_INPUT_MANIFEST_SHA256)
    message(FATAL_ERROR
        "configure input inventory changed after configuration; rerun CMake")
endif()
file(STRINGS "${_input_manifest}" _manifest_lines ENCODING UTF-8)
if(NOT _manifest_lines)
    message(FATAL_ERROR "configure input manifest is empty")
endif()
list(LENGTH CB_INPUT_RELATIVE _relative_count)
list(LENGTH CB_INPUT_SHA256 _hash_count)
if(NOT _manifest_lines STREQUAL CB_INPUT_RELATIVE OR
   NOT _relative_count EQUAL _hash_count)
    message(FATAL_ERROR
        "configure input inventory differs from the configuration snapshot; rerun CMake")
endif()

# Rebuild in a fresh private tree.  No generated file can leak into or alter
# SOURCE_DIR, and removing one declared product repairs the whole atomic
# contract instead of trusting stale Make dependency state.
file(REMOVE_RECURSE "${_binary_dir}")
set(_stage "${_binary_dir}/source")
set(_build "${_binary_dir}/build")
file(MAKE_DIRECTORY "${_stage}" "${_build}")
_cb_real_path("${_stage}" _stage)
_cb_real_path("${_build}" _build)
cmake_path(IS_PREFIX _binary_dir "${_stage}" NORMALIZE _stage_owned)
cmake_path(IS_PREFIX _binary_dir "${_build}" NORMALIZE _build_owned)
if(NOT _stage_owned OR NOT _build_owned)
    message(FATAL_ERROR "configure runner staging escaped its private binary directory")
endif()
set(_seen_paths "")
set(_input_index 0)
foreach(_line IN LISTS _manifest_lines)
    if(NOT _line MATCHES "^([A-Za-z0-9_.+/-]+)$")
        message(FATAL_ERROR "malformed configure input-manifest line '${_line}'")
    endif()
    set(_relative "${CMAKE_MATCH_1}")
    list(GET CB_INPUT_SHA256 ${_input_index} _digest)
    math(EXPR _input_index "${_input_index} + 1")
    string(LENGTH "${_digest}" _digest_length)
    if(NOT _digest_length EQUAL 64 OR
       _relative MATCHES "(^|/)[.][.]?(/|$)" OR
       IS_ABSOLUTE "${_relative}" OR _relative IN_LIST _seen_paths)
        message(FATAL_ERROR "unsafe configure input-manifest line '${_line}'")
    endif()
    list(APPEND _seen_paths "${_relative}")
    set(_source "${_source_dir}/${_relative}")
    set(_destination "${_stage}/${_relative}")
    cmake_path(NORMAL_PATH _source)
    cmake_path(NORMAL_PATH _destination)
    if(NOT EXISTS "${_source}" OR IS_DIRECTORY "${_source}" OR
       IS_SYMLINK "${_source}")
        message(FATAL_ERROR "configure input ${_relative} is missing or escaped")
    endif()
    _cb_real_path("${_source}" _source_real)
    cmake_path(IS_PREFIX _source_dir "${_source_real}" NORMALIZE _source_owned)
    if(NOT _source_owned)
        message(FATAL_ERROR "configure input ${_relative} is missing or escaped")
    endif()
    file(SHA256 "${_source_real}" _actual_input_sha256)
    if(NOT _actual_input_sha256 STREQUAL _digest)
        message(FATAL_ERROR
            "configure input ${_relative} changed after configuration; rerun CMake")
    endif()
    cmake_path(GET _destination PARENT_PATH _destination_parent)
    file(MAKE_DIRECTORY "${_destination_parent}")
    _cb_real_path("${_destination_parent}" _destination_parent)
    cmake_path(IS_PREFIX _stage "${_destination_parent}" NORMALIZE _destination_owned)
    if(NOT _destination_owned OR IS_SYMLINK "${_destination}")
        message(FATAL_ERROR "configure staging destination ${_relative} escaped its private tree")
    endif()
    file(COPY_FILE "${_source_real}" "${_destination}" ONLY_IF_DIFFERENT)
endforeach()

if(CB_MODE MATCHES "^adflib-")
    file(WRITE "${_stage}/src/config.h"
        "#ifndef AROS_ADFLIB_CONFIG_H\n#define AROS_ADFLIB_CONFIG_H 1\n#endif\n")
    set(_adflib_sources
        src/adf_hd.c
        src/adf_disk.c
        src/adf_raw.c
        src/adf_bitm.c
        src/adf_dump.c
        src/adf_util.c
        src/adf_env.c
        src/adf_dir.c
        src/adf_file.c
        src/adf_cache.c
        src/adf_link.c
        src/adf_salv.c
        src/generic/adf_nativ.c)
    set(_objects "")
    foreach(_relative IN LISTS _adflib_sources)
        cmake_path(GET _relative STEM _stem)
        set(_object "${_build}/${_stem}.o")
        _cb_run("compiling ADFlib ${_relative}"
            "${CB_COMPILER}"
            ${CB_COMPILE_FLAGS}
            -D_XOPEN_SOURCE
            -D_SVID_SOURCE
            -D_BSD_SOURCE
            -D_DEFAULT_SOURCE
            -D_GNU_SOURCE
            -std=c99
            "-I${_stage}/src"
            "-I${_stage}/src/generic"
            -c "${_stage}/${_relative}" -o "${_object}")
        list(APPEND _objects "${_object}")
    endforeach()
    set(_private_archive "${_build}/libadf.a")
    _cb_run("archiving ADFlib"
        "${CMAKE_COMMAND}" -E env ZERO_AR_DATE=1
        "${CB_ARCHIVER}" qc "${_private_archive}" ${_objects})
    _cb_run("indexing ADFlib"
        "${CMAKE_COMMAND}" -E env ZERO_AR_DATE=1
        "${CB_RANLIB}" "${_private_archive}")

    file(MAKE_DIRECTORY
        "${_install_prefix}/lib"
        "${_install_prefix}/include"
        "${_install_prefix}/lib/pkgconfig")
    foreach(_directory IN ITEMS
            "${_install_prefix}/lib"
            "${_install_prefix}/include"
            "${_install_prefix}/lib/pkgconfig")
        _cb_real_path("${_directory}" _directory_real)
        cmake_path(IS_PREFIX _install_prefix "${_directory_real}"
            NORMALIZE _directory_owned)
        if(NOT _directory_owned OR IS_SYMLINK "${_directory}")
            message(FATAL_ERROR "ADFlib install directory escaped its prefix")
        endif()
    endforeach()
    file(COPY_FILE "${_private_archive}"
        "${_install_prefix}/lib/libadf.a" ONLY_IF_DIFFERENT)
    set(_headers
        adf_defs.h adf_blk.h adf_err.h adf_str.h adflib.h adf_bitm.h
        adf_cache.h adf_dir.h adf_disk.h adf_dump.h adf_env.h adf_file.h
        adf_hd.h adf_link.h adf_raw.h adf_salv.h adf_util.h defendian.h
        hd_blk.h prefix.h)
    foreach(_header IN LISTS _headers)
        file(COPY_FILE "${_stage}/src/${_header}"
            "${_install_prefix}/include/${_header}" ONLY_IF_DIFFERENT)
    endforeach()
    file(COPY_FILE "${_stage}/src/generic/adf_nativ.h"
        "${_install_prefix}/include/adf_nativ.h" ONLY_IF_DIFFERENT)
    file(WRITE "${_install_prefix}/lib/pkgconfig/adflib.pc"
        "prefix=${_install_prefix}\n"
        "exec_prefix=${_install_prefix}\n"
        "libdir=${_install_prefix}/lib\n"
        "includedir=${_install_prefix}/include\n\n"
        "Name: adflib\n"
        "Description: Portable Amiga disk image library\n"
        "Version: 0.7.12\n"
        "Libs: -L${_install_prefix}/lib -ladf\n"
        "Cflags: -I${_install_prefix}/include\n")
else()
    set(_wireless_dir "${_stage}/wpa_supplicant")
    _cb_run("configuring WirelessManager"
        "${CB_SHELL}" "${_wireless_dir}/configure")

    # GNU Make stores CC as command text. Quote every token so an include or
    # tool path containing whitespace stays one compiler argument when the
    # recipe is expanded by /bin/sh.
    set(_cc_parts "${CB_COMPILER}" ${CB_COMPILE_FLAGS})
    set(_cc_command "")
    foreach(_part IN LISTS _cc_parts)
        string(REPLACE "'" "'\\''" _quoted "${_part}")
        string(APPEND _cc_command " '${_quoted}'")
    endforeach()
    string(STRIP "${_cc_command}" _cc_command)
    # The Makefile's own `LIBS += -lmui` is replaced, not extended, by a make
    # command-line variable, so exactly one archive may arrive here and it has
    # to be a real file. Element 0 of an unchecked list would otherwise take
    # whatever the contract happened to carry.
    list(LENGTH CB_DEPENDENCY_PRODUCTS _dependency_count)
    if(NOT _dependency_count EQUAL 1)
        message(FATAL_ERROR
            "WirelessManager expects one dependency archive, got ${_dependency_count}")
    endif()
    list(GET CB_DEPENDENCY_PRODUCTS 0 _mui_archive)
    if(NOT EXISTS "${_mui_archive}")
        message(FATAL_ERROR "WirelessManager dependency is missing: ${_mui_archive}")
    endif()
    file(SIZE "${_mui_archive}" _mui_size)
    if(_mui_size EQUAL 0)
        message(FATAL_ERROR "WirelessManager dependency is empty: ${_mui_archive}")
    endif()
    _cb_run("building WirelessManager"
        "${CB_MAKE}" -j4 -C "${_wireless_dir}"
        "CC=${_cc_command}"
        "LDO=${CB_LINKER}"
        "LDFLAGS=-r"
        "LIBS=${_mui_archive}"
        "LIBS_p="
        "LIBS_c=")
    file(MAKE_DIRECTORY "${_install_prefix}/C")
    _cb_real_path("${_install_prefix}/C" _wireless_install_dir)
    cmake_path(IS_PREFIX _install_prefix "${_wireless_install_dir}"
        NORMALIZE _wireless_install_owned)
    if(NOT _wireless_install_owned OR IS_SYMLINK "${_install_prefix}/C")
        message(FATAL_ERROR "WirelessManager install directory escaped its prefix")
    endif()
    file(COPY_FILE "${_wireless_dir}/wpa_supplicant"
        "${_install_prefix}/C/WirelessManager" ONLY_IF_DIFFERENT)
endif()

foreach(_product IN LISTS _private_products _install_products)
    if(NOT EXISTS "${_product}" OR IS_DIRECTORY "${_product}")
        message(FATAL_ERROR
            "configure-style build did not create declared product ${_product}")
    endif()
endforeach()
