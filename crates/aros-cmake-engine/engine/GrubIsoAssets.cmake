include_guard(GLOBAL)

include(CMakeParseArguments)

# Closed GRUB 2.12 BIOS/EFI asset staging.  This deliberately stops at the
# files a PC ISO needs below SYS/: it does not create grub.cfg, invoke an ISO
# composer, or claim to build the native AROS GRUB utilities.
set(_AROS_GRUB_ISO_ASSETS_MODULE_DIR "${CMAKE_CURRENT_LIST_DIR}")
set(_AROS_GRUB_ISO_ASSETS_HOST_MMAKE_RELATIVE
    "arch/all-pc/boot/grub2-host/mmakefile.src")
set(_AROS_GRUB_ISO_ASSETS_PC_MANIFEST "cmake/manifests/grub-2.12-pc.install")
set(_AROS_GRUB_ISO_ASSETS_EFI64_MANIFEST "cmake/manifests/grub-2.12-efi64.install")
set(_AROS_GRUB_ISO_ASSETS_EFI32_MANIFEST "cmake/manifests/grub-2.12-efi32.install")

function(_aros_grub_iso_assets_safe_value label value)
    foreach(_needle IN ITEMS ";" "\"" "\n" "\r" "$" "[" "]")
        string(FIND "${value}" "${_needle}" _position)
        if(NOT _position EQUAL -1)
            message(FATAL_ERROR "${label} contains an unsafe value")
        endif()
    endforeach()
endfunction()

# Resolves an existing prefix physically but keeps a non-existent tail.  This
# accepts macOS' /tmp spelling while still exposing a symlink escape.
function(_aros_grub_iso_assets_real_path path output)
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

function(_aros_grub_iso_assets_reject_symlink_components root path label)
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

function(_aros_grub_iso_assets_require_regular path label)
    if(NOT EXISTS "${path}" OR IS_DIRECTORY "${path}" OR IS_SYMLINK "${path}")
        message(FATAL_ERROR "${label} must be an existing regular file")
    endif()
endfunction()

function(_aros_grub_iso_assets_validate_relative label relative)
    if(relative STREQUAL "" OR relative MATCHES "^/" OR
       relative MATCHES "(^|/)\\.\\.(/|$)" OR relative MATCHES "\\\\" OR
       relative MATCHES ";")
        message(FATAL_ERROR "${label} contains an unsafe relative path: ${relative}")
    endif()
endfunction()

function(_aros_grub_iso_assets_write_if_changed path content)
    if(EXISTS "${path}")
        file(READ "${path}" _previous)
    else()
        set(_previous "")
    endif()
    if(NOT "${_previous}" STREQUAL "${content}")
        file(WRITE "${path}" "${content}")
    endif()
endfunction()

# Selects exactly the installed products the historic ISO recipe copies.  The
# complete, sorted install manifests are a checked-in static inventory; no
# globbing or host discovery participates in CMake's output ownership.
function(_aros_grub_iso_assets_collect_manifest
         source_root manifest_relative platform expected_mods
         expected_images output)
    _aros_grub_iso_assets_validate_relative("GRUB2 ISO manifest" "${manifest_relative}")
    set(_manifest "${source_root}/${manifest_relative}")
    _aros_grub_iso_assets_require_regular("${_manifest}" "GRUB2 ISO manifest")
    file(STRINGS "${_manifest}" _entries)
    set(_sorted_entries "${_entries}")
    list(SORT _sorted_entries)
    if(NOT "${_entries}" STREQUAL "${_sorted_entries}")
        message(FATAL_ERROR "GRUB2 ISO manifest ${manifest_relative} is not sorted")
    endif()

    set(_selected "")
    set(_module_count 0)
    set(_image_count 0)
    set(_list_count 0)
    set(_mkimage_count 0)
    foreach(_relative IN LISTS _entries)
        _aros_grub_iso_assets_validate_relative("GRUB2 ISO manifest entry" "${_relative}")
        if(_relative STREQUAL "grub-mkimage")
            math(EXPR _mkimage_count "${_mkimage_count} + 1")
        elseif(_relative MATCHES "^lib/grub/${platform}/[^/]+\\.mod$")
            list(APPEND _selected "${_relative}")
            math(EXPR _module_count "${_module_count} + 1")
        elseif(_relative MATCHES "^lib/grub/${platform}/[^/]+\\.img$")
            # EFI install trees contain kernel.img as a host-side product, but
            # the legacy ISO recipe intentionally copies only modules and
            # lists from them.  BIOS is the sole lane whose installed images
            # are staged wholesale.
            if(NOT expected_images EQUAL 0)
                list(APPEND _selected "${_relative}")
                math(EXPR _image_count "${_image_count} + 1")
            endif()
        elseif(_relative MATCHES "^lib/grub/${platform}/(command|fs|moddep)\\.lst$")
            list(APPEND _selected "${_relative}")
            math(EXPR _list_count "${_list_count} + 1")
        endif()
    endforeach()
    if(NOT _mkimage_count EQUAL 1 OR NOT _module_count EQUAL expected_mods OR
       NOT _image_count EQUAL expected_images OR NOT _list_count EQUAL 3)
        message(FATAL_ERROR
            "GRUB2 ISO manifest ${manifest_relative} does not match the audited asset inventory")
    endif()
    list(SORT _selected)
    set(${output} "${_selected}" PARENT_SCOPE)
endfunction()

# aros_stage_grub2_iso_assets(
#     MODE x86_64
#     BINARY_DIR <private ${build}/gen/grub2-iso-assets/x86_64>
#     SYS_DIR <${build}/SYS>
#     HOST_PC <${build}/hosttools/grub2/pc>
#     HOST_EFI64 <${build}/hosttools/grub2/efi-x86_64>
#     HOST_EFI32 <${build}/hosttools/grub2/efi-i386>)
#
# The fixed identity makes a second declaration or a substituted host prefix a
# configuration error.  The public target is intentionally new and does not
# change legacy grub2-iso-setup semantics while native grub2-aros is absent.
function(aros_stage_grub2_iso_assets)
    set(_one_value_args MODE BINARY_DIR SYS_DIR HOST_PC HOST_EFI64 HOST_EFI32)
    cmake_parse_arguments(GIA "" "${_one_value_args}" "" ${ARGN})
    if(GIA_UNPARSED_ARGUMENTS OR GIA_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "aros_stage_grub2_iso_assets received malformed arguments")
    endif()
    foreach(_required IN LISTS _one_value_args)
        if(NOT GIA_${_required})
            message(FATAL_ERROR "aros_stage_grub2_iso_assets requires ${_required}")
        endif()
    endforeach()
    if(NOT GIA_MODE STREQUAL "x86_64")
        message(FATAL_ERROR "GRUB2 ISO assets only support the audited x86_64-pc mode")
    endif()
    if(TARGET aros-grub2-iso-assets)
        message(FATAL_ERROR "GRUB2 ISO asset target was already declared")
    endif()
    foreach(_host_target IN ITEMS grub2-host grub2-efi-host grub2-efi32-host)
        if(NOT TARGET "${_host_target}")
            message(FATAL_ERROR
                "GRUB2 ISO assets require the real host-lane target ${_host_target}")
        endif()
    endforeach()

    foreach(_name IN LISTS _one_value_args)
        _aros_grub_iso_assets_safe_value("GRUB2 ISO ${_name}" "${GIA_${_name}}")
    endforeach()
    if(DEFINED AROS_GRUB_SOURCE_ROOT)
        set(_source_root_input "${AROS_GRUB_SOURCE_ROOT}")
    else()
        set(_source_root_input "${CMAKE_SOURCE_DIR}")
    endif()
    _aros_grub_iso_assets_safe_value("GRUB2 ISO source root" "${_source_root_input}")

    cmake_path(ABSOLUTE_PATH _source_root_input NORMALIZE
        OUTPUT_VARIABLE _source_root_logical)
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE
        OUTPUT_VARIABLE _build_root_logical)
    if(NOT EXISTS "${_source_root_logical}" OR NOT IS_DIRECTORY "${_source_root_logical}")
        message(FATAL_ERROR "GRUB2 ISO source root is unavailable")
    endif()
    _aros_grub_iso_assets_real_path("${_source_root_logical}" _source_root)
    _aros_grub_iso_assets_real_path("${_build_root_logical}" _build_root)
    if(IS_SYMLINK "${_source_root_logical}" OR IS_SYMLINK "${_build_root_logical}")
        message(FATAL_ERROR "GRUB2 ISO source or build root may not be a symlink")
    endif()

    set(_host_mmake_logical
        "${_source_root_logical}/${_AROS_GRUB_ISO_ASSETS_HOST_MMAKE_RELATIVE}")
    _aros_grub_iso_assets_require_regular("${_host_mmake_logical}" "GRUB2 ISO source recipe")
    _aros_grub_iso_assets_real_path("${_host_mmake_logical}" _host_mmake)
    set(_expected_host_mmake
        "${_source_root}/${_AROS_GRUB_ISO_ASSETS_HOST_MMAKE_RELATIVE}")
    _aros_grub_iso_assets_real_path("${_expected_host_mmake}" _expected_host_mmake)
    cmake_path(IS_PREFIX _source_root "${_host_mmake}" NORMALIZE _host_mmake_owned)
    if(NOT _host_mmake_owned OR NOT _host_mmake STREQUAL _expected_host_mmake)
        message(FATAL_ERROR "GRUB2 ISO source recipe has an unsupported location")
    endif()

    set(_binary_input "${GIA_BINARY_DIR}")
    set(_sys_input "${GIA_SYS_DIR}")
    set(_pc_input "${GIA_HOST_PC}")
    set(_efi64_input "${GIA_HOST_EFI64}")
    set(_efi32_input "${GIA_HOST_EFI32}")
    cmake_path(ABSOLUTE_PATH _binary_input BASE_DIRECTORY "${_build_root_logical}"
        NORMALIZE OUTPUT_VARIABLE _binary_logical)
    cmake_path(ABSOLUTE_PATH _sys_input BASE_DIRECTORY "${_build_root_logical}"
        NORMALIZE OUTPUT_VARIABLE _sys_logical)
    cmake_path(ABSOLUTE_PATH _pc_input BASE_DIRECTORY "${_build_root_logical}"
        NORMALIZE OUTPUT_VARIABLE _host_pc_logical)
    cmake_path(ABSOLUTE_PATH _efi64_input BASE_DIRECTORY "${_build_root_logical}"
        NORMALIZE OUTPUT_VARIABLE _host_efi64_logical)
    cmake_path(ABSOLUTE_PATH _efi32_input BASE_DIRECTORY "${_build_root_logical}"
        NORMALIZE OUTPUT_VARIABLE _host_efi32_logical)
    # Inspect the requested spelling before resolving it.  A symlink that
    # happens to point back inside the build tree must not silently become an
    # alternative owner for this closed output layout.
    foreach(_pair IN ITEMS
            "binary|${_binary_logical}" "sys|${_sys_logical}"
            "host-pc|${_host_pc_logical}" "host-efi64|${_host_efi64_logical}"
            "host-efi32|${_host_efi32_logical}")
        string(REPLACE "|" ";" _pair_parts "${_pair}")
        list(GET _pair_parts 0 _label)
        list(GET _pair_parts 1 _path)
        _aros_grub_iso_assets_reject_symlink_components(
            "${_build_root_logical}" "${_path}" "GRUB2 ISO ${_label} path")
    endforeach()
    _aros_grub_iso_assets_real_path("${_binary_logical}" _binary_dir)
    _aros_grub_iso_assets_real_path("${_sys_logical}" _sys_dir)
    _aros_grub_iso_assets_real_path("${_host_pc_logical}" _host_pc)
    _aros_grub_iso_assets_real_path("${_host_efi64_logical}" _host_efi64)
    _aros_grub_iso_assets_real_path("${_host_efi32_logical}" _host_efi32)

    set(_expected_binary_logical "${_build_root_logical}/gen/grub2-iso-assets/x86_64")
    set(_expected_sys_logical "${_build_root_logical}/SYS")
    set(_expected_pc_logical "${_build_root_logical}/hosttools/grub2/pc")
    set(_expected_efi64_logical "${_build_root_logical}/hosttools/grub2/efi-x86_64")
    set(_expected_efi32_logical "${_build_root_logical}/hosttools/grub2/efi-i386")
    _aros_grub_iso_assets_real_path("${_expected_binary_logical}" _expected_binary)
    _aros_grub_iso_assets_real_path("${_expected_sys_logical}" _expected_sys)
    _aros_grub_iso_assets_real_path("${_expected_pc_logical}" _expected_pc)
    _aros_grub_iso_assets_real_path("${_expected_efi64_logical}" _expected_efi64)
    _aros_grub_iso_assets_real_path("${_expected_efi32_logical}" _expected_efi32)
    if(NOT _binary_dir STREQUAL _expected_binary OR NOT _sys_dir STREQUAL _expected_sys OR
       NOT _host_pc STREQUAL _expected_pc OR NOT _host_efi64 STREQUAL _expected_efi64 OR
       NOT _host_efi32 STREQUAL _expected_efi32)
        message(FATAL_ERROR "GRUB2 ISO asset path identity differs from the audited layout")
    endif()
    foreach(_pair IN ITEMS
            "binary|${_binary_dir}" "sys|${_sys_dir}" "host-pc|${_host_pc}"
            "host-efi64|${_host_efi64}" "host-efi32|${_host_efi32}")
        string(REPLACE "|" ";" _pair_parts "${_pair}")
        list(GET _pair_parts 0 _label)
        list(GET _pair_parts 1 _path)
        cmake_path(IS_PREFIX _build_root "${_path}" NORMALIZE _owned)
        if(NOT _owned OR _path STREQUAL _build_root)
            message(FATAL_ERROR "GRUB2 ISO ${_label} path escapes the build tree")
        endif()
        _aros_grub_iso_assets_reject_symlink_components(
            "${_build_root}" "${_path}" "GRUB2 ISO ${_label} path")
    endforeach()
    cmake_path(IS_PREFIX _binary_dir "${_sys_dir}" NORMALIZE _binary_contains_sys)
    cmake_path(IS_PREFIX _sys_dir "${_binary_dir}" NORMALIZE _sys_contains_binary)
    if(_binary_contains_sys OR _sys_contains_binary)
        message(FATAL_ERROR "GRUB2 ISO asset private and SYS roots overlap")
    endif()

    _aros_grub_iso_assets_collect_manifest(
        "${_source_root}" "${_AROS_GRUB_ISO_ASSETS_PC_MANIFEST}"
        "i386-pc" 273 8 _pc_products)
    _aros_grub_iso_assets_collect_manifest(
        "${_source_root}" "${_AROS_GRUB_ISO_ASSETS_EFI64_MANIFEST}"
        "x86_64-efi" 268 0 _efi64_products)
    _aros_grub_iso_assets_collect_manifest(
        "${_source_root}" "${_AROS_GRUB_ISO_ASSETS_EFI32_MANIFEST}"
        "i386-efi" 269 0 _efi32_products)

    set(_inputs_logical
        "${_host_pc_logical}/grub-mkimage"
        "${_host_efi64_logical}/grub-mkimage"
        "${_host_efi32_logical}/grub-mkimage")
    set(_products_logical "")
    set(_products_physical "")
    foreach(_relative IN LISTS _pc_products)
        string(REGEX REPLACE "^lib/grub/i386-pc/" "" _name "${_relative}")
        list(APPEND _inputs_logical "${_host_pc_logical}/${_relative}")
        list(APPEND _products_logical "${_sys_logical}/boot/grub/i386-pc/${_name}")
        list(APPEND _products_physical "${_sys_dir}/boot/grub/i386-pc/${_name}")
    endforeach()
    list(APPEND _products_logical
        "${_sys_logical}/boot/grub/i386-pc/core.img"
        "${_sys_logical}/boot/grub/i386-pc/grub2_eltorito")
    list(APPEND _products_physical
        "${_sys_dir}/boot/grub/i386-pc/core.img"
        "${_sys_dir}/boot/grub/i386-pc/grub2_eltorito")
    foreach(_relative IN LISTS _efi64_products)
        string(REGEX REPLACE "^lib/grub/x86_64-efi/" "" _name "${_relative}")
        list(APPEND _inputs_logical "${_host_efi64_logical}/${_relative}")
        list(APPEND _products_logical "${_sys_logical}/EFI/BOOT/grub/x86_64-efi/${_name}")
        list(APPEND _products_physical "${_sys_dir}/EFI/BOOT/grub/x86_64-efi/${_name}")
    endforeach()
    list(APPEND _products_logical "${_sys_logical}/EFI/BOOT/BOOTX64.EFI")
    list(APPEND _products_physical "${_sys_dir}/EFI/BOOT/BOOTX64.EFI")
    foreach(_relative IN LISTS _efi32_products)
        string(REGEX REPLACE "^lib/grub/i386-efi/" "" _name "${_relative}")
        list(APPEND _inputs_logical "${_host_efi32_logical}/${_relative}")
        list(APPEND _products_logical "${_sys_logical}/EFI/BOOT/grub/i386-efi/${_name}")
        list(APPEND _products_physical "${_sys_dir}/EFI/BOOT/grub/i386-efi/${_name}")
    endforeach()
    list(APPEND _products_logical "${_sys_logical}/EFI/BOOT/BOOTIA32.EFI")
    list(APPEND _products_physical "${_sys_dir}/EFI/BOOT/BOOTIA32.EFI")
    list(APPEND _products_logical "${_binary_logical}/grub2.mods")
    list(APPEND _products_physical "${_binary_dir}/grub2.mods")
    list(REMOVE_DUPLICATES _inputs_logical)
    list(REMOVE_DUPLICATES _products_logical)
    list(REMOVE_DUPLICATES _products_physical)
    list(LENGTH _products_logical _product_count)
    if(NOT _product_count EQUAL 832)
        message(FATAL_ERROR "GRUB2 ISO asset inventory has ${_product_count} products, expected 832")
    endif()

    set(_registered_outputs "")
    get_property(_registered_outputs GLOBAL PROPERTY AROS_GRUB2_ISO_ASSET_OUTPUTS)
    foreach(_product IN LISTS _products_physical)
        if(_product IN_LIST _registered_outputs)
            message(FATAL_ERROR "GRUB2 ISO asset product is already owned: ${_product}")
        endif()
        list(APPEND _registered_outputs "${_product}")
    endforeach()
    set_property(GLOBAL PROPERTY AROS_GRUB2_ISO_ASSET_OUTPUTS "${_registered_outputs}")

    set(_stamp_logical "${_binary_logical}/.grub2-iso-assets.stamp")
    set(_stamp "${_binary_dir}/.grub2-iso-assets.stamp")
    set(_contract_logical "${_build_root_logical}/.aros-grub2-iso-assets-contract.cmake")
    set(_contract "${_build_root}/.aros-grub2-iso-assets-contract.cmake")
    _aros_grub_iso_assets_reject_symlink_components(
        "${_build_root}" "${_contract}" "GRUB2 ISO contract path")
    string(CONCAT _contract_content
        "# Generated closed GRUB2 ISO asset contract.  Do not edit.\n"
        "set(GIA_MODE [==[${GIA_MODE}]==])\n"
        "set(GIA_SOURCE_ROOT [==[${_source_root}]==])\n"
        "set(GIA_BUILD_ROOT [==[${_build_root}]==])\n"
        "set(GIA_BINARY_DIR [==[${_binary_dir}]==])\n"
        "set(GIA_SYS_DIR [==[${_sys_dir}]==])\n"
        "set(GIA_HOST_PC [==[${_host_pc}]==])\n"
        "set(GIA_HOST_EFI64 [==[${_host_efi64}]==])\n"
        "set(GIA_HOST_EFI32 [==[${_host_efi32}]==])\n"
        "set(GIA_STAMP [==[${_stamp}]==])\n")
    foreach(_product IN LISTS _products_physical)
        _aros_grub_iso_assets_safe_value("GRUB2 ISO product" "${_product}")
        string(APPEND _contract_content "list(APPEND GIA_PRODUCTS [==[${_product}]==])\n")
    endforeach()
    _aros_grub_iso_assets_write_if_changed("${_contract}" "${_contract_content}")

    add_custom_command(
        OUTPUT ${_products_logical} "${_stamp_logical}"
        COMMAND "${CMAKE_COMMAND}" -DGIA_ACTION=stage
            "-DCONTRACT=${_contract_logical}"
            -P "${_AROS_GRUB_ISO_ASSETS_MODULE_DIR}/RunGrubIsoAssets.cmake"
        DEPENDS ${_inputs_logical}
            "${_host_mmake_logical}"
            "${_source_root_logical}/${_AROS_GRUB_ISO_ASSETS_PC_MANIFEST}"
            "${_source_root_logical}/${_AROS_GRUB_ISO_ASSETS_EFI64_MANIFEST}"
            "${_source_root_logical}/${_AROS_GRUB_ISO_ASSETS_EFI32_MANIFEST}"
            "${_contract_logical}"
            "${_AROS_GRUB_ISO_ASSETS_MODULE_DIR}/RunGrubIsoAssets.cmake"
        COMMENT "Staging audited GRUB2 BIOS and EFI ISO assets"
        VERBATIM)
    add_custom_target(aros-grub2-iso-assets
        DEPENDS ${_products_logical} "${_stamp_logical}")
    # These are deliberately direct target edges, not historic quick aliases
    # that currently resolve only to the shared source fetch endpoint.
    add_dependencies(aros-grub2-iso-assets
        grub2-host grub2-efi-host grub2-efi32-host)
    set_property(GLOBAL PROPERTY AROS_GRUB2_ISO_ASSET_PRODUCTS "${_products_logical}")
endfunction()
