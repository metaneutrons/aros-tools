include_guard(GLOBAL)

include(CMakeParseArguments)
include("${CMAKE_CURRENT_LIST_DIR}/LinklibArchive.cmake")

# Resolve existing path components physically, including the common macOS
# /tmp -> /private/tmp symlink, then append a non-existing tail.  CMake's
# file(REAL_PATH) leaves that tail lexically rooted on some versions, which
# makes a safe private output look unrelated to its canonical parent.
function(_aros_configure_real_path path output)
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

# aros_build_configure(
#     MMAKE_ID <target>
#     MODE <adflib-host|adflib-target|wirelessmanager>
#     SOURCE_DIR <read-only-local-source-root>
#     BINARY_DIR <private-stage/build-root>
#     INSTALL_PREFIX <build-tree-prefix>
#     INPUT_MANIFEST <path-inventory>
#     PRIVATE_PRODUCTS <paths...>
#     INSTALL_PRODUCTS <paths...>
#     [DEPENDENCY_TARGETS <link-library targets...>]
#     [PROVIDED_LIBRARY <uselibs-name>])
#
# Closed counterpart for the three supported local `%build_with_configure`
# declarations.  The transpiler selects a capability and validates every source;
# this layer independently checks the same path/product shape, generates an
# immutable runner contract and tracks every private and installed output.
function(aros_build_configure)
    set(oneValueArgs MMAKE_ID MODE SOURCE_DIR BINARY_DIR INSTALL_PREFIX
        INPUT_MANIFEST PROVIDED_LIBRARY)
    set(multiValueArgs PRIVATE_PRODUCTS INSTALL_PRODUCTS
        DEPENDENCY_TARGETS)
    cmake_parse_arguments(PARSE_ARGV 0 CB "" "${oneValueArgs}" "${multiValueArgs}")

    if(CB_UNPARSED_ARGUMENTS OR CB_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "aros_build_configure received malformed arguments")
    endif()
    foreach(_required IN ITEMS MMAKE_ID MODE SOURCE_DIR BINARY_DIR INSTALL_PREFIX
            INPUT_MANIFEST)
        if(NOT CB_${_required})
            message(FATAL_ERROR "aros_build_configure requires ${_required}")
        endif()
    endforeach()
    if(NOT CB_PRIVATE_PRODUCTS OR NOT CB_INSTALL_PRODUCTS)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: configure-style build requires private and installed products")
    endif()
    if(NOT CB_MMAKE_ID MATCHES "^[A-Za-z0-9_.+-]+$")
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: invalid configure-style target name")
    endif()
    if(NOT CB_MODE MATCHES "^(adflib-host|adflib-target|wirelessmanager)$")
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: unsupported configure-style mode ${CB_MODE}")
    endif()
    if(TARGET "${CB_MMAKE_ID}")
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: configure-style target was already declared")
    endif()

    if(DEFINED AROS_CONFIGURE_SOURCE_ROOT)
        set(_source_root_raw "${AROS_CONFIGURE_SOURCE_ROOT}")
    else()
        set(_source_root_raw "${CMAKE_SOURCE_DIR}")
    endif()
    cmake_path(ABSOLUTE_PATH _source_root_raw NORMALIZE
        OUTPUT_VARIABLE _source_root_lexical)
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE
        OUTPUT_VARIABLE _build_root_lexical)
    foreach(_name IN ITEMS SOURCE_DIR BINARY_DIR INSTALL_PREFIX INPUT_MANIFEST)
        set(_raw "${CB_${_name}}")
        if(_raw MATCHES "[;\"$\\\r\n]")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: unsafe configure-style ${_name} path '${_raw}'")
        endif()
        string(TOLOWER "${_name}" _lower)
        set(_path "${_raw}")
        cmake_path(ABSOLUTE_PATH _path BASE_DIRECTORY "${_build_root_lexical}"
            NORMALIZE OUTPUT_VARIABLE _${_lower}_lexical)
    endforeach()

    # Resolve every pre-existing component before doing any ownership check.
    # Lexical NORMALIZE alone accepts e.g. gen/configure -> /tmp, which would
    # make the runner's recursive cleanup and staging escape the build tree.
    if(NOT EXISTS "${_source_root_lexical}" OR
       NOT IS_DIRECTORY "${_source_root_lexical}")
        message(FATAL_ERROR "${CB_MMAKE_ID}: configure source root is unavailable")
    endif()
    if(NOT EXISTS "${_source_dir_lexical}" OR
       NOT IS_DIRECTORY "${_source_dir_lexical}")
        message(FATAL_ERROR "${CB_MMAKE_ID}: source directory is unavailable")
    endif()
    if(NOT EXISTS "${_input_manifest_lexical}" OR
       IS_DIRECTORY "${_input_manifest_lexical}" OR
       IS_SYMLINK "${_input_manifest_lexical}")
        message(FATAL_ERROR "${CB_MMAKE_ID}: input manifest must be an existing file")
    endif()
    _aros_configure_real_path("${_source_root_lexical}" _source_root)
    _aros_configure_real_path("${_build_root_lexical}" _build_root)
    _aros_configure_real_path("${_source_dir_lexical}" _source_dir)
    _aros_configure_real_path("${_input_manifest_lexical}" _input_manifest)
    _aros_configure_real_path("${_binary_dir_lexical}" _binary_dir)
    _aros_configure_real_path("${_install_prefix_lexical}" _install_prefix)

    cmake_path(IS_PREFIX _source_root "${_source_dir}" NORMALIZE _source_owned)
    if(NOT _source_owned OR _source_dir STREQUAL _source_root)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: source must be a private child of the source tree")
    endif()
    cmake_path(IS_PREFIX _source_dir "${_input_manifest}" NORMALIZE _manifest_owned)
    if(NOT _manifest_owned OR _input_manifest STREQUAL _source_dir OR
       NOT EXISTS "${_input_manifest}" OR IS_DIRECTORY "${_input_manifest}")
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: input manifest must be a file below SOURCE_DIR")
    endif()
    set(_configure_root "${_build_root}/gen/configure")
    _aros_configure_real_path("${_configure_root}" _configure_root)
    cmake_path(IS_PREFIX _build_root "${_configure_root}" NORMALIZE _configure_root_owned)
    if(NOT _configure_root_owned OR _configure_root STREQUAL _build_root)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: configure root escapes the build tree")
    endif()
    cmake_path(IS_PREFIX _configure_root "${_binary_dir}" NORMALIZE _binary_owned)
    if(NOT _binary_owned OR _binary_dir STREQUAL _configure_root)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: binary directory must be a private child of ${_configure_root}")
    endif()
    cmake_path(IS_PREFIX _build_root "${_install_prefix}" NORMALIZE _prefix_owned)
    if(NOT _prefix_owned OR _install_prefix STREQUAL _build_root)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: install prefix must be a private child of the build tree")
    endif()
    foreach(_owned IN ITEMS source_dir input_manifest install_prefix)
        set(_owner_path "${_${_owned}}")
        cmake_path(IS_PREFIX _binary_dir "${_owner_path}" NORMALIZE _binary_contains)
        cmake_path(IS_PREFIX _owner_path "${_binary_dir}" NORMALIZE _owner_contains)
        if(_binary_contains OR _owner_contains)
            message(FATAL_ERROR
            "${CB_MMAKE_ID}: binary directory overlaps ${_owned}")
        endif()
    endforeach()

    # The parser admits exactly three legacy invocations.  Keep the CMake
    # boundary equally closed: an approved MODE cannot be re-used with a
    # different target name, source subtree, manifest, build lane or prefix.
    if(CB_MODE STREQUAL "adflib-host")
        set(_expected_mmake_id "host-adflib")
        set(_expected_source_relative "tools/ADFlib")
        set(_expected_manifest_relative "tools/ADFlib/adflib-configure.inputs")
        set(_expected_binary_relative "gen/configure/tools/ADFlib/host")
        set(_expected_prefix_relative "hosttools")
        set(_expected_provided_library "")
    elseif(CB_MODE STREQUAL "adflib-target")
        set(_expected_mmake_id "linklib-adflib")
        set(_expected_source_relative "tools/ADFlib")
        set(_expected_manifest_relative "tools/ADFlib/adflib-configure.inputs")
        set(_expected_binary_relative "gen/configure/tools/ADFlib/target")
        set(_expected_prefix_relative "SYS/Developer")
        set(_expected_provided_library "adf")
    else()
        set(_expected_mmake_id "workbench-network-wirelessmanager")
        set(_expected_source_relative "workbench/network/WirelessManager")
        set(_expected_manifest_relative
            "workbench/network/WirelessManager/wirelessmanager-configure.inputs")
        set(_expected_binary_relative
            "gen/configure/workbench/network/WirelessManager")
        set(_expected_prefix_relative "SYS")
        set(_expected_provided_library "")
    endif()
    _aros_configure_real_path("${_source_root}/${_expected_source_relative}"
        _expected_source_dir)
    _aros_configure_real_path("${_source_root}/${_expected_manifest_relative}"
        _expected_input_manifest)
    set(_expected_binary_dir "${_build_root}/${_expected_binary_relative}")
    set(_expected_install_prefix "${_build_root}/${_expected_prefix_relative}")
    _aros_configure_real_path("${_expected_binary_dir}" _expected_binary_dir)
    _aros_configure_real_path("${_expected_install_prefix}" _expected_install_prefix)
    if(NOT CB_MMAKE_ID STREQUAL _expected_mmake_id)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: target identity differs from the audited ${CB_MODE} capability")
    endif()
    if(NOT _source_dir STREQUAL _expected_source_dir)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: source identity differs from the audited capability")
    endif()
    if(NOT _input_manifest STREQUAL _expected_input_manifest)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: input-manifest identity differs from the supported capability")
    endif()
    if(NOT _binary_dir STREQUAL _expected_binary_dir)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: binary identity differs from the audited capability")
    endif()
    if(NOT _install_prefix STREQUAL _expected_install_prefix)
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: install-prefix identity differs from the audited capability")
    endif()
    if(NOT "${CB_PROVIDED_LIBRARY}" STREQUAL "${_expected_provided_library}")
        message(FATAL_ERROR
            "${CB_MMAKE_ID}: provided-library identity differs from the audited capability")
    endif()

    file(SHA256 "${_input_manifest}" _actual_manifest_sha256)
    file(STRINGS "${_input_manifest}" _manifest_lines ENCODING UTF-8)
    if(NOT _manifest_lines)
        message(FATAL_ERROR "${CB_MMAKE_ID}: input manifest is empty")
    endif()
    set(_input_files "")
    set(_manifest_paths "")
    set(_input_hashes "")
    foreach(_line IN LISTS _manifest_lines)
        if(NOT _line MATCHES "^([A-Za-z0-9_.+/-]+)$")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: malformed input-manifest line '${_line}'")
        endif()
        set(_relative "${CMAKE_MATCH_1}")
        if(_relative MATCHES "(^|/)[.][.]?(/|$)" OR
           IS_ABSOLUTE "${_relative}")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: unsafe input-manifest line '${_line}'")
        endif()
        if(_relative IN_LIST _manifest_paths)
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: repeated input-manifest path ${_relative}")
        endif()
        list(APPEND _manifest_paths "${_relative}")
        set(_input "${_source_dir}/${_relative}")
        cmake_path(NORMAL_PATH _input)
        if(NOT EXISTS "${_input}" OR IS_DIRECTORY "${_input}" OR
           IS_SYMLINK "${_input}")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: missing or escaped configure input ${_relative}")
        endif()
        _aros_configure_real_path("${_input}" _input_real)
        cmake_path(IS_PREFIX _source_dir "${_input_real}" NORMALIZE _input_owned)
        if(NOT _input_owned)
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: missing or escaped configure input ${_relative}")
        endif()
        file(SHA256 "${_input_real}" _actual_input_sha256)
        list(APPEND _input_files "${_input_real}")
        list(APPEND _input_hashes "${_actual_input_sha256}")
    endforeach()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
        "${_input_manifest}" ${_input_files})

    set(_private_products "")
    set(_install_products "")
    set(_private_build_products "")
    set(_install_build_products "")
    foreach(_kind IN ITEMS PRIVATE INSTALL)
        foreach(_raw_product IN LISTS CB_${_kind}_PRODUCTS)
            if(_raw_product MATCHES "[;\"$\\\r\n]")
                message(FATAL_ERROR
                    "${CB_MMAKE_ID}: unsafe configure product '${_raw_product}'")
            endif()
            set(_product "${_raw_product}")
            cmake_path(ABSOLUTE_PATH _product BASE_DIRECTORY "${_build_root}"
                NORMALIZE OUTPUT_VARIABLE _product_lexical)
            _aros_configure_real_path("${_product_lexical}" _product)
            if(_kind STREQUAL "PRIVATE")
                cmake_path(IS_PREFIX _binary_dir "${_product}" NORMALIZE _owned)
                if(NOT _owned OR _product STREQUAL _binary_dir)
                    message(FATAL_ERROR
                        "${CB_MMAKE_ID}: private product escapes binary directory: ${_product}")
                endif()
                list(APPEND _private_products "${_product}")
                list(APPEND _private_build_products "${_product_lexical}")
            else()
                cmake_path(IS_PREFIX _install_prefix "${_product}" NORMALIZE _owned)
                if(NOT _owned OR _product STREQUAL _install_prefix)
                    message(FATAL_ERROR
                        "${CB_MMAKE_ID}: installed product escapes prefix: ${_product}")
                endif()
                list(APPEND _install_products "${_product}")
                list(APPEND _install_build_products "${_product_lexical}")
            endif()
        endforeach()
    endforeach()
    set(_all_products ${_private_products} ${_install_products})
    set(_all_build_products
        ${_private_build_products} ${_install_build_products})
    list(LENGTH _all_products _declared_product_count)
    list(REMOVE_DUPLICATES _all_products)
    list(LENGTH _all_products _unique_product_count)
    if(NOT _declared_product_count EQUAL _unique_product_count)
        message(FATAL_ERROR "${CB_MMAKE_ID}: duplicate configure product")
    endif()
    set(_dependency_products "")
    # Keep the caller spelling for CMake's dependency graph. Ninja keys
    # outputs textually, so on macOS an upstream /tmp output is not the same
    # graph node as its physical /private/tmp counterpart. The runner contract
    # below receives only the separately checked physical path.
    set(_dependency_build_dependencies "")
    # DEPENDENCY_TARGETS names link libraries and asks each target where it
    # writes. The path used to be spelled out, here and in the transpiler's
    # declaration, as `<build root>/liblinklibs-mui.a`; both went stale when
    # linklibs-mui became canonical, and the build only kept working because a
    # file from an earlier configuration was still lying in the build root
    # (OPEN-POINTS 44).
    set(_dependency_paths "")
    foreach(_dependency_target IN LISTS CB_DEPENDENCY_TARGETS)
        aros_linklib_archive_path("${_dependency_target}" _dependency_archive)
        list(APPEND _dependency_paths "${_dependency_archive}")
    endforeach()
    foreach(_raw_dependency IN LISTS _dependency_paths)
        if(_raw_dependency MATCHES "[;\"$\\\r\n]")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: unsafe configure dependency '${_raw_dependency}'")
        endif()
        set(_dependency "${_raw_dependency}")
        cmake_path(ABSOLUTE_PATH _dependency BASE_DIRECTORY "${_build_root}"
            NORMALIZE OUTPUT_VARIABLE _dependency_lexical)
        _aros_configure_real_path("${_dependency_lexical}" _dependency)
        cmake_path(IS_PREFIX _build_root "${_dependency}" NORMALIZE _dependency_owned)
        if(NOT _dependency_owned OR _dependency STREQUAL _build_root)
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: configure dependency escapes the build tree: ${_dependency}")
        endif()
        list(APPEND _dependency_products "${_dependency}")
        list(APPEND _dependency_build_dependencies "${_dependency_lexical}")
    endforeach()

    set(_adflib_headers
        adf_defs.h adf_blk.h adf_err.h adf_str.h adflib.h adf_bitm.h
        adf_cache.h adf_dir.h adf_disk.h adf_dump.h adf_env.h adf_file.h
        adf_hd.h adf_link.h adf_raw.h adf_salv.h adf_util.h defendian.h
        hd_blk.h prefix.h adf_nativ.h)
    if(CB_MODE MATCHES "^adflib-")
        set(_expected_private "${_binary_dir}/build/libadf.a")
        set(_expected_install "${_install_prefix}/lib/libadf.a")
        foreach(_header IN LISTS _adflib_headers)
            list(APPEND _expected_install "${_install_prefix}/include/${_header}")
        endforeach()
        list(APPEND _expected_install "${_install_prefix}/lib/pkgconfig/adflib.pc")
        if(NOT "${_private_products}" STREQUAL "${_expected_private}" OR
           NOT "${_install_products}" STREQUAL "${_expected_install}")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: ADFlib product contract differs from the audited capability")
        endif()
        if(CB_MODE STREQUAL "adflib-host" AND CB_PROVIDED_LIBRARY)
            message(FATAL_ERROR "${CB_MMAKE_ID}: host ADFlib cannot publish a target library")
        elseif(CB_MODE STREQUAL "adflib-target" AND
               NOT CB_PROVIDED_LIBRARY STREQUAL "adf")
            message(FATAL_ERROR "${CB_MMAKE_ID}: target ADFlib must provide adf")
        endif()
        if(_dependency_products)
            message(FATAL_ERROR "${CB_MMAKE_ID}: ADFlib has no external build products")
        endif()
    else()
        set(_expected_private
            "${_binary_dir}/source/wpa_supplicant/wpa_supplicant"
            "${_binary_dir}/source/wpa_supplicant/wpa_passphrase"
            "${_binary_dir}/source/wpa_supplicant/wpa_cli")
        set(_expected_install "${_install_prefix}/C/WirelessManager")
        # The audited fact is which link library wpa_supplicant links, not
        # where that library keeps its archive: this expectation was the third
        # place spelling `<build root>/liblinklibs-mui.a`, and it went stale
        # with the other two.
        set(_expected_dependency_targets "linklibs-mui")
        list(LENGTH _dependency_products _resolved_dependency_count)
        if(NOT "${_private_products}" STREQUAL "${_expected_private}" OR
           NOT "${_install_products}" STREQUAL "${_expected_install}" OR
           NOT "${CB_DEPENDENCY_TARGETS}" STREQUAL "${_expected_dependency_targets}" OR
           NOT _resolved_dependency_count EQUAL 1 OR
           CB_PROVIDED_LIBRARY)
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: WirelessManager product/dependency contract differs from the audited capability")
        endif()
    endif()

    foreach(_product IN LISTS _all_products)
        string(SHA256 _product_key "${_product}")
        get_property(_previous_owner GLOBAL PROPERTY
            "AROS_CONFIGURE_PRODUCT_OWNER_${_product_key}")
        if(_previous_owner AND NOT _previous_owner STREQUAL CB_MMAKE_ID)
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: ${_product} is already owned by ${_previous_owner}")
        endif()
        set_property(GLOBAL PROPERTY
            "AROS_CONFIGURE_PRODUCT_OWNER_${_product_key}" "${CB_MMAKE_ID}")
    endforeach()

    get_directory_property(_parent_options COMPILE_OPTIONS)
    get_directory_property(_parent_definitions COMPILE_DEFINITIONS)
    get_directory_property(_parent_includes INCLUDE_DIRECTORIES)
    set(_target_flags -O2)
    foreach(_option IN LISTS _parent_options)
        if(_option STREQUAL "$<$<COMPILE_LANGUAGE:CXX>:-nostdinc++>")
            # ConfigureBuild's audited capabilities compile C only.
            continue()
        endif()
        if(_option MATCHES "[;\r\n]" OR _option MATCHES "^\\$<")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: unsupported parent compile option '${_option}'")
        endif()
        list(APPEND _target_flags "${_option}")
    endforeach()
    foreach(_definition IN LISTS _parent_definitions)
        if(_definition MATCHES "[;\r\n]" OR _definition MATCHES "^\\$<")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: unsupported parent compile definition '${_definition}'")
        endif()
        list(APPEND _target_flags "-D${_definition}")
    endforeach()
    if(CB_MODE STREQUAL "adflib-target")
        list(APPEND _target_flags
            "-I${AROS_SDK_INCLUDE_DIR}/aros/posixc"
            "-I${AROS_SDK_INCLUDE_DIR}/aros/stdc")
    endif()
    foreach(_include IN LISTS _parent_includes)
        if(_include STREQUAL
           "$<$<COMPILE_LANGUAGE:CXX>:${AROS_CROSS_TOOLCHAIN_ROOT}/include/c++/v1>")
            # This configure-style capability compiles C only. The parent
            # graph's language-scoped libc++ include root does not apply.
            continue()
        endif()
        if(_include MATCHES "[;\r\n]" OR _include MATCHES "^\\$<")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: unsupported parent include '${_include}'")
        endif()
        list(APPEND _target_flags "-I${_include}")
    endforeach()
    list(APPEND _target_flags -Wno-unused-parameter)
    if(CB_MODE STREQUAL "wirelessmanager")
        list(APPEND _target_flags
            -Wno-implicit-function-declaration -Wno-int-conversion)
    endif()

    if(CB_MODE STREQUAL "adflib-host")
        if(NOT AROS_HOST_CC)
            set(_compiler cc)
        else()
            set(_compiler "${AROS_HOST_CC}")
        endif()
        find_program(_archiver NAMES ar REQUIRED)
        find_program(_ranlib NAMES ranlib REQUIRED)
        set(_compile_flags -O2 -Wno-unused-parameter)
    else()
        set(_compiler "${CMAKE_C_COMPILER}")
        set(_archiver "${CMAKE_AR}")
        set(_ranlib "${CMAKE_RANLIB}")
        set(_compile_flags ${_target_flags})
    endif()
    find_program(_make_program NAMES gmake make REQUIRED)
    find_program(_shell_program NAMES sh REQUIRED)
    if(CB_MODE STREQUAL "wirelessmanager")
        if(NOT AROS_LLD_BIN)
            find_program(_linker_program NAMES ld.lld REQUIRED)
        else()
            set(_linker_program "${AROS_LLD_BIN}")
        endif()
    endif()

    set(_contract "${CMAKE_CURRENT_BINARY_DIR}/.aros-${CB_MMAKE_ID}-configure-contract.cmake")
    set(_contract_content "")
    set(_contract_pairs
            "CB_MODE|${CB_MODE}"
            "CB_SOURCE_ROOT|${_source_root}"
            "CB_BUILD_ROOT|${_build_root}"
            "CB_SOURCE_DIR|${_source_dir}"
            "CB_BINARY_DIR|${_binary_dir}"
            "CB_INSTALL_PREFIX|${_install_prefix}"
            "CB_INPUT_MANIFEST|${_input_manifest}"
            "CB_INPUT_MANIFEST_SHA256|${_actual_manifest_sha256}"
            "CB_COMPILER|${_compiler}"
            "CB_ARCHIVER|${_archiver}"
            "CB_RANLIB|${_ranlib}"
            "CB_MAKE|${_make_program}"
            "CB_SHELL|${_shell_program}")
    if(CB_MODE STREQUAL "wirelessmanager")
        list(APPEND _contract_pairs "CB_LINKER|${_linker_program}")
    endif()
    foreach(_pair IN LISTS _contract_pairs)
        string(FIND "${_pair}" "|" _separator)
        string(SUBSTRING "${_pair}" 0 ${_separator} _name)
        math(EXPR _value_start "${_separator} + 1")
        string(SUBSTRING "${_pair}" ${_value_start} -1 _value)
        string(APPEND _contract_content "set(${_name} [==[${_value}]==])\n")
    endforeach()
    foreach(_list_name IN ITEMS CB_PRIVATE_PRODUCTS CB_INSTALL_PRODUCTS
            CB_DEPENDENCY_PRODUCTS CB_COMPILE_FLAGS CB_INPUT_RELATIVE
            CB_INPUT_SHA256)
        string(APPEND _contract_content "set(${_list_name})\n")
    endforeach()
    foreach(_value IN LISTS _private_products)
        string(APPEND _contract_content
            "list(APPEND CB_PRIVATE_PRODUCTS [==[${_value}]==])\n")
    endforeach()
    foreach(_value IN LISTS _install_products)
        string(APPEND _contract_content
            "list(APPEND CB_INSTALL_PRODUCTS [==[${_value}]==])\n")
    endforeach()
    foreach(_value IN LISTS _dependency_products)
        string(APPEND _contract_content
            "list(APPEND CB_DEPENDENCY_PRODUCTS [==[${_value}]==])\n")
    endforeach()
    foreach(_value IN LISTS _compile_flags)
        string(APPEND _contract_content
            "list(APPEND CB_COMPILE_FLAGS [==[${_value}]==])\n")
    endforeach()
    foreach(_value IN LISTS _manifest_paths)
        string(APPEND _contract_content
            "list(APPEND CB_INPUT_RELATIVE [==[${_value}]==])\n")
    endforeach()
    foreach(_value IN LISTS _input_hashes)
        string(APPEND _contract_content
            "list(APPEND CB_INPUT_SHA256 [==[${_value}]==])\n")
    endforeach()
    file(GENERATE OUTPUT "${_contract}" CONTENT "${_contract_content}")

    set(_runner "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunConfigureBuild.cmake")
    # CMake/Ninja identifies outputs by their declared spelling, while the
    # runner above operates only on their physical counterparts. Preserve the
    # former here so a /tmp producer can satisfy a /tmp dependency on macOS.
    set(_stamp "${_binary_dir_lexical}/.aros-${CB_MMAKE_ID}-installed")
    add_custom_command(
        OUTPUT "${_stamp}" ${_all_build_products}
        COMMAND "${CMAKE_COMMAND}" "-DCONTRACT=${_contract}" -P "${_runner}"
        COMMAND "${CMAKE_COMMAND}" -E touch "${_stamp}"
        DEPENDS "${_runner}" "${_contract}" "${_input_manifest}"
            ${_input_files} ${_dependency_build_dependencies}
        COMMENT "Building configure-style target ${CB_MMAKE_ID}"
        VERBATIM
        COMMAND_EXPAND_LISTS)
    add_custom_target("${CB_MMAKE_ID}"
        DEPENDS "${_stamp}" ${_all_build_products})

    if(CB_PROVIDED_LIBRARY)
        set(_provider "${CB_MMAKE_ID}-configure-${CB_PROVIDED_LIBRARY}")
        if(TARGET "${_provider}")
            message(FATAL_ERROR
                "${CB_MMAKE_ID}: configure library provider already exists")
        endif()
        set(_archive
            "${_install_prefix_lexical}/lib/lib${CB_PROVIDED_LIBRARY}.a")
        if(COMMAND _aros_claim_linklib_archive)
            _aros_claim_linklib_archive(
                "${CB_MMAKE_ID}" "${_install_prefix_lexical}/lib"
                "${CB_PROVIDED_LIBRARY}")
        endif()
        add_library("${_provider}" INTERFACE)
        add_dependencies("${_provider}" "${CB_MMAKE_ID}")
        target_link_libraries("${_provider}" INTERFACE "${_archive}")
        target_include_directories("${_provider}" INTERFACE
            "${_install_prefix_lexical}/include")
    endif()
endfunction()
