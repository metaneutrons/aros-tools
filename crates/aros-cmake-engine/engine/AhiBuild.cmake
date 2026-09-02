include_guard(GLOBAL)

include(CMakeParseArguments)
include("${CMAKE_CURRENT_LIST_DIR}/LinklibArchive.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/Executable.cmake")

# Closed CMake boundary for AHI's remaining %build_with_configure declaration.
# It is intentionally not included from AROS.cmake until the transpiler selects
# this capability and emits its public target edge.
set(_AROS_AHI_MMAKE_ID "workbench-devs-AHI-subsystem")

# Resolve existing components physically, including /tmp -> /private/tmp, and
# append a non-existing tail.  Graph paths remain lexical; contracts use these
# physical values only for containment and runner-side filesystem operations.
function(_aros_ahi_real_path path output)
    set(_candidate "${path}")
    cmake_path(ABSOLUTE_PATH _candidate NORMALIZE OUTPUT_VARIABLE _candidate)
    set(_tail "")
    while(NOT EXISTS "${_candidate}" AND NOT IS_SYMLINK "${_candidate}")
        cmake_path(GET _candidate FILENAME _part)
        cmake_path(GET _candidate PARENT_PATH _parent)
        if(_part STREQUAL "" OR _parent STREQUAL _candidate)
            message(FATAL_ERROR "AHI: cannot resolve physical path ${path}")
        endif()
        list(PREPEND _tail "${_part}")
        set(_candidate "${_parent}")
    endwhile()
    if(IS_SYMLINK "${_candidate}" AND NOT EXISTS "${_candidate}")
        message(FATAL_ERROR "AHI: refusing dangling symlink in ${path}")
    endif()
    file(REAL_PATH "${_candidate}" _resolved)
    foreach(_part IN LISTS _tail)
        set(_resolved "${_resolved}/${_part}")
    endforeach()
    cmake_path(NORMAL_PATH _resolved)
    set(${output} "${_resolved}" PARENT_SCOPE)
endfunction()

function(_aros_ahi_safe_value name value)
    if("${value}" MATCHES "[;\"$\\\\\r\n]" OR "${value}" MATCHES "==\\]")
        message(FATAL_ERROR "AHI: unsafe ${name} '${value}'")
    endif()
endfunction()

# Autoconf substitutes these values into Makefiles, where a literal space in
# a path becomes a separate shell word.  This closed capability deliberately
# rejects such paths instead of relying on brittle, make-specific escaping.
function(_aros_ahi_require_make_path name value)
    _aros_ahi_safe_value("${name}" "${value}")
    if("${value}" MATCHES "[ \t\r\n]")
        message(FATAL_ERROR
            "AHI: ${name} cannot contain whitespace for configure/Make")
    endif()
endfunction()

function(_aros_ahi_require_executable name raw output)
    _aros_ahi_require_make_path("${name}" "${raw}")
    if(NOT IS_ABSOLUTE "${raw}")
        message(FATAL_ERROR "AHI: ${name} must be an absolute path")
    endif()
    cmake_path(NORMAL_PATH raw OUTPUT_VARIABLE _lexical)
    aros_path_is_executable("${_lexical}" _lexical_executable)
    if(NOT _lexical_executable)
        message(FATAL_ERROR "AHI: ${name} is not an executable regular file")
    endif()
    _aros_ahi_real_path("${_lexical}" _physical)
    aros_path_is_executable("${_physical}" _physical_executable)
    if(NOT _physical_executable)
        message(FATAL_ERROR "AHI: ${name} resolved to a non-executable file")
    endif()
    _aros_ahi_require_make_path("${name}" "${_physical}")
    # Execute through the configured lexical path.  Some LLVM utilities are
    # one binary behind argv[0]-selecting symlinks (llvm-ranlib and
    # llvm-strip), so resolving them here would silently turn them into
    # llvm-ar/llvm-objcopy.
    set(${output} "${_lexical}" PARENT_SCOPE)
endfunction()

# lld selects its linker personality from argv[0].  Keep the checked lexical
# ld.lld path for execution instead of resolving Homebrew's ld.lld -> lld
# symlink, while still proving that its final referent is a safe executable.
function(_aros_ahi_require_ld_lld name raw output)
    _aros_ahi_require_make_path("${name}" "${raw}")
    if(NOT IS_ABSOLUTE "${raw}")
        message(FATAL_ERROR "AHI: ${name} must be an absolute path")
    endif()
    cmake_path(NORMAL_PATH raw OUTPUT_VARIABLE _lexical)
    cmake_path(GET _lexical FILENAME _program_name)
    if(NOT _program_name STREQUAL "ld.lld")
        message(FATAL_ERROR "AHI: ${name} must invoke ld.lld by that exact name")
    endif()
    aros_path_is_executable("${_lexical}" _lexical_executable)
    if(NOT _lexical_executable)
        message(FATAL_ERROR "AHI: ${name} is not an executable regular file")
    endif()
    _aros_ahi_real_path("${_lexical}" _physical)
    aros_path_is_executable("${_physical}" _physical_executable)
    if(NOT _physical_executable)
        message(FATAL_ERROR "AHI: ${name} resolved to a non-executable file")
    endif()
    _aros_ahi_require_make_path("${name}" "${_physical}")
    set(${output} "${_lexical}" PARENT_SCOPE)
endfunction()

# Quote an already validated path for the generated POSIX-shell adapter.  The
# AHI configure boundary records its tool paths as plain Make words, but the
# adapter itself embeds two absolute paths and therefore must also handle a
# literal apostrophe without turning it into shell syntax.
function(_aros_ahi_shell_quote value output)
    string(REPLACE "'" "'\"'\"'" _escaped "${value}")
    set(${output} "'${_escaped}'" PARENT_SCOPE)
endfunction()

function(_aros_ahi_host_triplet output)
    if(DEFINED AROS_AHI_BUILD_TRIPLET)
        set(_triplet "${AROS_AHI_BUILD_TRIPLET}")
    elseif(CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin" AND
           CMAKE_HOST_SYSTEM_PROCESSOR MATCHES "^(arm64|aarch64)$")
        set(_triplet "aarch64-apple-darwin")
    elseif(CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin" AND
           CMAKE_HOST_SYSTEM_PROCESSOR STREQUAL "x86_64")
        set(_triplet "x86_64-apple-darwin")
    elseif(CMAKE_HOST_SYSTEM_NAME STREQUAL "Linux" AND
           CMAKE_HOST_SYSTEM_PROCESSOR STREQUAL "x86_64")
        set(_triplet "x86_64-pc-linux-gnu")
    elseif(CMAKE_HOST_SYSTEM_NAME STREQUAL "Linux" AND
           CMAKE_HOST_SYSTEM_PROCESSOR MATCHES "^(arm64|aarch64)$")
        set(_triplet "aarch64-unknown-linux-gnu")
    else()
        message(FATAL_ERROR
            "AHI: unknown configured host identity; set AROS_AHI_BUILD_TRIPLET")
    endif()
    if(NOT _triplet MATCHES "^[A-Za-z0-9_.+-]+$")
        message(FATAL_ERROR "AHI: unsafe build triplet '${_triplet}'")
    endif()
    set(${output} "${_triplet}" PARENT_SCOPE)
endfunction()

# aros_build_ahi(
#   MMAKE_ID workbench-devs-AHI-subsystem
#   MODE <x86_64|arm|aarch64>
#   BINARY_DIR <private>
#   INSTALL_PREFIX <SYS>
#   HOST_SFDC <absolute-output>
#   HOST_PERL <absolute>)
#
# The other inputs are explicit configured values, never discoveries:
# AROS_AHI_MAKE (absolute GNU make), AROS_AHI_FLEXCAT or AROS_HOST_FLEXCAT,
# the CMake target tools, and <build>/SDK/include.
function(aros_build_ahi)
    set(_one MMAKE_ID MODE BINARY_DIR INSTALL_PREFIX HOST_SFDC HOST_PERL)
    cmake_parse_arguments(PARSE_ARGV 0 AB "" "${_one}" "")
    if(AB_UNPARSED_ARGUMENTS OR AB_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "aros_build_ahi received malformed arguments")
    endif()
    foreach(_name IN LISTS _one)
        if(NOT AB_${_name})
            message(FATAL_ERROR "aros_build_ahi requires ${_name}")
        endif()
    endforeach()
    if(NOT AB_MMAKE_ID STREQUAL _AROS_AHI_MMAKE_ID)
        message(FATAL_ERROR "AHI: target identity must be ${_AROS_AHI_MMAKE_ID}")
    endif()
    if(NOT AB_MODE MATCHES "^(x86_64|arm|aarch64)$")
        message(FATAL_ERROR "AHI: unsupported mode ${AB_MODE}")
    endif()
    if(TARGET "${AB_MMAKE_ID}")
        message(FATAL_ERROR "AHI: ${AB_MMAKE_ID} was already declared")
    endif()

    if(DEFINED AROS_AHI_SOURCE_ROOT)
        set(_source_root_raw "${AROS_AHI_SOURCE_ROOT}")
    else()
        set(_source_root_raw "${CMAKE_SOURCE_DIR}")
    endif()
    set(_build_root_raw "${CMAKE_BINARY_DIR}")
    foreach(_raw IN ITEMS _source_root_raw _build_root_raw)
        _aros_ahi_require_make_path("${_raw}" "${${_raw}}")
        cmake_path(ABSOLUTE_PATH ${_raw} NORMALIZE OUTPUT_VARIABLE ${_raw}_lexical)
    endforeach()
    if(NOT EXISTS "${_source_root_raw_lexical}" OR
       NOT IS_DIRECTORY "${_source_root_raw_lexical}" OR
       NOT EXISTS "${_build_root_raw_lexical}" OR
       NOT IS_DIRECTORY "${_build_root_raw_lexical}")
        message(FATAL_ERROR "AHI: source or build root is unavailable")
    endif()
    set(_source_root_lexical "${_source_root_raw_lexical}")
    set(_build_root_lexical "${_build_root_raw_lexical}")
    _aros_ahi_real_path("${_source_root_lexical}" _source_root)
    _aros_ahi_real_path("${_build_root_lexical}" _build_root)

    foreach(_name IN ITEMS BINARY_DIR INSTALL_PREFIX HOST_SFDC HOST_PERL)
        _aros_ahi_require_make_path("${_name}" "${AB_${_name}}")
        if(NOT IS_ABSOLUTE "${AB_${_name}}")
            message(FATAL_ERROR "AHI: ${_name} must be an absolute path")
        endif()
        set(_value "${AB_${_name}}")
        cmake_path(NORMAL_PATH _value OUTPUT_VARIABLE _${_name}_lexical)
    endforeach()
    _aros_ahi_real_path("${_BINARY_DIR_lexical}" _binary_dir)
    _aros_ahi_real_path("${_INSTALL_PREFIX_lexical}" _install_prefix)
    _aros_ahi_real_path("${_HOST_SFDC_lexical}" _host_sfdc)
    _aros_ahi_require_executable("HOST_PERL" "${_HOST_PERL_lexical}" _host_perl)
    foreach(_path IN ITEMS _source_root _build_root _binary_dir _install_prefix
            _host_sfdc _host_perl)
        _aros_ahi_require_make_path("${_path}" "${${_path}}")
    endforeach()

    set(_source_dir_lexical "${_source_root_lexical}/workbench/devs/AHI")
    set(_source_manifest_lexical "${_source_dir_lexical}/ahi-build.inputs")
    set(_product_manifest_lexical
        "${_source_root_lexical}/cmake/manifests/ahi-${AB_MODE}.install")
    set(_expected_binary_lexical
        "${_build_root_lexical}/gen/configure/workbench/devs/AHI/${AB_MODE}")
    set(_expected_prefix_lexical "${_build_root_lexical}/SYS")
    set(_expected_sfdc_lexical "${_build_root_lexical}/hosttools/sfdc")
    foreach(_path IN ITEMS _source_dir_lexical _source_manifest_lexical
            _product_manifest_lexical _expected_binary_lexical
            _expected_prefix_lexical _expected_sfdc_lexical)
        cmake_path(NORMAL_PATH ${_path})
    endforeach()
    if(NOT EXISTS "${_source_dir_lexical}" OR NOT IS_DIRECTORY "${_source_dir_lexical}" OR
       NOT EXISTS "${_source_manifest_lexical}" OR IS_DIRECTORY "${_source_manifest_lexical}" OR
       IS_SYMLINK "${_source_manifest_lexical}" OR
       NOT EXISTS "${_product_manifest_lexical}" OR IS_DIRECTORY "${_product_manifest_lexical}" OR
       IS_SYMLINK "${_product_manifest_lexical}")
        message(FATAL_ERROR "AHI: audited source or manifest is unavailable")
    endif()
    _aros_ahi_real_path("${_source_dir_lexical}" _source_dir)
    _aros_ahi_real_path("${_source_manifest_lexical}" _source_manifest)
    _aros_ahi_real_path("${_product_manifest_lexical}" _product_manifest)
    _aros_ahi_real_path("${_expected_binary_lexical}" _expected_binary)
    _aros_ahi_real_path("${_expected_prefix_lexical}" _expected_prefix)
    _aros_ahi_real_path("${_expected_sfdc_lexical}" _expected_sfdc)
    cmake_path(IS_PREFIX _source_root "${_source_dir}" NORMALIZE _source_owned)
    cmake_path(IS_PREFIX _source_dir "${_source_manifest}" NORMALIZE _source_manifest_owned)
    cmake_path(IS_PREFIX _source_root "${_product_manifest}" NORMALIZE _product_manifest_owned)
    cmake_path(IS_PREFIX _build_root "${_binary_dir}" NORMALIZE _binary_owned)
    cmake_path(IS_PREFIX _build_root "${_install_prefix}" NORMALIZE _prefix_owned)
    cmake_path(IS_PREFIX _build_root "${_host_sfdc}" NORMALIZE _sfdc_owned)
    if(NOT _source_owned OR _source_dir STREQUAL _source_root OR
       NOT _source_manifest_owned OR NOT _product_manifest_owned OR
       NOT _binary_owned OR _binary_dir STREQUAL _build_root OR
       NOT _prefix_owned OR _install_prefix STREQUAL _build_root OR NOT _sfdc_owned)
        message(FATAL_ERROR "AHI: audited paths escape their owning tree")
    endif()
    if(NOT _source_dir STREQUAL "${_source_root}/workbench/devs/AHI" OR
       NOT _source_manifest STREQUAL "${_source_dir}/ahi-build.inputs" OR
       NOT _product_manifest STREQUAL
           "${_source_root}/cmake/manifests/ahi-${AB_MODE}.install" OR
       NOT _binary_dir STREQUAL _expected_binary OR
       NOT _install_prefix STREQUAL _expected_prefix OR
       NOT _host_sfdc STREQUAL _expected_sfdc)
        message(FATAL_ERROR
            "AHI: source, build, install or host-SFDC identity differs from the audited capability")
    endif()
    foreach(_other IN ITEMS _source_dir _source_manifest _install_prefix _product_manifest)
        cmake_path(IS_PREFIX _binary_dir "${${_other}}" NORMALIZE _inside_binary)
        set(_other_value "${${_other}}")
        cmake_path(IS_PREFIX _other_value "${_binary_dir}" NORMALIZE _contains_binary)
        if(_inside_binary OR _contains_binary)
            message(FATAL_ERROR "AHI: private BINARY_DIR overlaps ${_other}")
        endif()
    endforeach()

    file(SHA256 "${_source_manifest}" _source_manifest_hash)
    file(SHA256 "${_product_manifest}" _product_manifest_hash)

    file(STRINGS "${_source_manifest}" _source_lines ENCODING UTF-8)
    if(NOT _source_lines)
        message(FATAL_ERROR "AHI: source input manifest is empty")
    endif()
    set(_input_relative "")
    set(_input_hashes "")
    set(_input_dependencies "")
    foreach(_line IN LISTS _source_lines)
        if(NOT _line MATCHES "^(.+)$")
            message(FATAL_ERROR "AHI: malformed source input-manifest line '${_line}'")
        endif()
        set(_relative "${CMAKE_MATCH_1}")
        if(IS_ABSOLUTE "${_relative}" OR _relative MATCHES "(^|/)[.][.]?(/|$)" OR
           _relative MATCHES "[;\"$\\\\\r\n]" OR _relative MATCHES "==\\]" OR
           _relative IN_LIST _input_relative)
            message(FATAL_ERROR "AHI: unsafe source input-manifest path '${_relative}'")
        endif()
        set(_input_lexical "${_source_dir_lexical}/${_relative}")
        cmake_path(NORMAL_PATH _input_lexical)
        if(NOT EXISTS "${_input_lexical}" OR IS_DIRECTORY "${_input_lexical}" OR
           IS_SYMLINK "${_input_lexical}")
            message(FATAL_ERROR "AHI: source input ${_relative} is missing or symlinked")
        endif()
        _aros_ahi_real_path("${_input_lexical}" _input)
        cmake_path(IS_PREFIX _source_dir "${_input}" NORMALIZE _input_owned)
        if(NOT _input_owned)
            message(FATAL_ERROR "AHI: source input ${_relative} escaped its source tree")
        endif()
        file(SHA256 "${_input}" _input_hash)
        list(APPEND _input_relative "${_relative}")
        list(APPEND _input_hashes "${_input_hash}")
        list(APPEND _input_dependencies "${_input_lexical}")
    endforeach()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
        "${_source_manifest_lexical}" ${_input_dependencies})

    file(STRINGS "${_product_manifest}" _product_lines ENCODING UTF-8)
    set(_product_relative "")
    set(_product_kinds "")
    set(_install_outputs_lexical "")
    set(_install_outputs "")
    foreach(_line IN LISTS _product_lines)
        if(NOT _line MATCHES "^(elf|data|mode)  (.+)$")
            message(FATAL_ERROR "AHI: malformed product-manifest line '${_line}'")
        endif()
        set(_kind "${CMAKE_MATCH_1}")
        set(_relative "${CMAKE_MATCH_2}")
        if(IS_ABSOLUTE "${_relative}" OR _relative MATCHES "(^|/)[.][.]?(/|$)" OR
           _relative MATCHES "[;\"$\\\\\r\n]" OR _relative MATCHES "==\\]" OR
           _relative IN_LIST _product_relative)
            message(FATAL_ERROR "AHI: unsafe product-manifest path '${_relative}'")
        endif()
        set(_product_lexical "${_INSTALL_PREFIX_lexical}/${_relative}")
        cmake_path(NORMAL_PATH _product_lexical)
        _aros_ahi_real_path("${_product_lexical}" _product)
        cmake_path(IS_PREFIX _install_prefix "${_product}" NORMALIZE _product_owned)
        if(NOT _product_owned OR _product STREQUAL _install_prefix)
            message(FATAL_ERROR "AHI: product ${_relative} escaped the install prefix")
        endif()
        list(APPEND _product_relative "${_relative}")
        list(APPEND _product_kinds "${_kind}")
        list(APPEND _install_outputs_lexical "${_product_lexical}")
        list(APPEND _install_outputs "${_product}")
    endforeach()
    list(LENGTH _product_relative _product_count)
    if((AB_MODE STREQUAL "x86_64" AND NOT _product_count EQUAL 73) OR
       ((AB_MODE STREQUAL "arm" OR AB_MODE STREQUAL "aarch64") AND
        NOT _product_count EQUAL 85))
        message(FATAL_ERROR "AHI: product count differs from audited ${AB_MODE} capability")
    endif()

    if(NOT AROS_AHI_MAKE)
        message(FATAL_ERROR "AHI: AROS_AHI_MAKE must name absolute GNU make")
    endif()
    _aros_ahi_require_executable("AROS_AHI_MAKE" "${AROS_AHI_MAKE}" _make)
    execute_process(
        COMMAND "${_make}" --version
        RESULT_VARIABLE _make_probe_result
        OUTPUT_VARIABLE _make_probe_stdout
        ERROR_VARIABLE _make_probe_stderr)
    if(NOT _make_probe_result EQUAL 0 OR
       NOT _make_probe_stdout MATCHES "GNU Make")
        message(FATAL_ERROR "AHI: AROS_AHI_MAKE must be GNU make")
    endif()
    if(AROS_AHI_FLEXCAT)
        set(_flexcat_raw "${AROS_AHI_FLEXCAT}")
    elseif(AROS_HOST_FLEXCAT)
        set(_flexcat_raw "${AROS_HOST_FLEXCAT}")
    else()
        message(FATAL_ERROR "AHI: AROS_AHI_FLEXCAT or AROS_HOST_FLEXCAT is required")
    endif()
    _aros_ahi_require_make_path("FlexCat" "${_flexcat_raw}")
    if(NOT IS_ABSOLUTE "${_flexcat_raw}")
        message(FATAL_ERROR "AHI: FlexCat must be an absolute path")
    endif()
    cmake_path(NORMAL_PATH _flexcat_raw OUTPUT_VARIABLE _flexcat_lexical)
    _aros_ahi_real_path("${_flexcat_lexical}" _flexcat)
    _aros_ahi_require_make_path("FlexCat" "${_flexcat}")
    set(_expected_flexcat_lexical "${_build_root_lexical}/hosttools/flexcat")
    cmake_path(NORMAL_PATH _expected_flexcat_lexical)
    _aros_ahi_real_path("${_expected_flexcat_lexical}" _expected_flexcat)
    if(NOT _flexcat STREQUAL _expected_flexcat)
        message(FATAL_ERROR "AHI: FlexCat must be the private hosttools/flexcat output")
    endif()

    foreach(_tool IN ITEMS CMAKE_C_COMPILER CMAKE_AR CMAKE_RANLIB CMAKE_OBJCOPY CMAKE_STRIP)
        if(NOT ${_tool})
            message(FATAL_ERROR "AHI: ${_tool} must be configured explicitly")
        endif()
        _aros_ahi_require_executable("${_tool}" "${${_tool}}" _tool_value)
        set(_${_tool} "${_tool_value}")
    endforeach()
    if(NOT AROS_COLLECT_BIN)
        message(FATAL_ERROR "AHI: AROS_COLLECT_BIN must be configured explicitly")
    endif()
    _aros_ahi_require_executable("AROS_COLLECT_BIN" "${AROS_COLLECT_BIN}" _collect)
    if(NOT AROS_AHI_RUNNER_BIN)
        message(FATAL_ERROR "AHI: AROS_AHI_RUNNER_BIN must be configured explicitly")
    endif()
    _aros_ahi_require_executable(
        "AROS_AHI_RUNNER_BIN" "${AROS_AHI_RUNNER_BIN}" _ahi_runner)
    if(NOT AROS_LLD_BIN)
        message(FATAL_ERROR "AHI: AROS_LLD_BIN must be configured explicitly")
    endif()
    _aros_ahi_require_ld_lld("AROS_LLD_BIN" "${AROS_LLD_BIN}" _lld)
    if(DEFINED AROS_AHI_SDK_INCLUDE_DIR)
        set(_sdk_raw "${AROS_AHI_SDK_INCLUDE_DIR}")
    else()
        set(_sdk_raw "${_build_root_lexical}/SDK/include")
    endif()
    _aros_ahi_require_make_path("SDK include directory" "${_sdk_raw}")
    if(NOT IS_ABSOLUTE "${_sdk_raw}")
        message(FATAL_ERROR "AHI: SDK include directory must be absolute")
    endif()
    cmake_path(NORMAL_PATH _sdk_raw OUTPUT_VARIABLE _sdk_lexical)
    if(NOT EXISTS "${_sdk_lexical}" OR NOT IS_DIRECTORY "${_sdk_lexical}")
        message(FATAL_ERROR "AHI: SDK include directory is unavailable")
    endif()
    _aros_ahi_real_path("${_sdk_lexical}" _sdk)
    _aros_ahi_require_make_path("SDK include directory" "${_sdk}")
    set(_expected_sdk_lexical "${_build_root_lexical}/SDK/include")
    cmake_path(NORMAL_PATH _expected_sdk_lexical)
    _aros_ahi_real_path("${_expected_sdk_lexical}" _expected_sdk)
    cmake_path(IS_PREFIX _build_root "${_sdk}" NORMALIZE _sdk_owned)
    if(NOT _sdk_owned OR NOT _sdk STREQUAL _expected_sdk)
        message(FATAL_ERROR "AHI: SDK include differs from the build SDK")
    endif()
    foreach(_sdk_layer IN ITEMS aros/posixc aros/stdc)
        set(_sdk_layer_lexical "${_sdk_lexical}/${_sdk_layer}")
        if(NOT EXISTS "${_sdk_layer_lexical}" OR NOT IS_DIRECTORY "${_sdk_layer_lexical}")
            message(FATAL_ERROR "AHI: SDK ${_sdk_layer} include directory is unavailable")
        endif()
        _aros_ahi_real_path("${_sdk_layer_lexical}" _sdk_layer_real)
        cmake_path(IS_PREFIX _sdk "${_sdk_layer_real}" NORMALIZE _sdk_layer_owned)
        if(NOT _sdk_layer_owned)
            message(FATAL_ERROR "AHI: SDK ${_sdk_layer} include directory escaped the SDK")
        endif()
    endforeach()
    set(_gen_include_lexical "${_build_root_lexical}/GENINCDIR")
    set(_mui_header_lexical "${_gen_include_lexical}/libraries/mui.h")
    cmake_path(NORMAL_PATH _gen_include_lexical)
    cmake_path(NORMAL_PATH _mui_header_lexical)
    if(NOT EXISTS "${_gen_include_lexical}" OR NOT IS_DIRECTORY "${_gen_include_lexical}" OR
       NOT EXISTS "${_mui_header_lexical}" OR IS_DIRECTORY "${_mui_header_lexical}" OR
       IS_SYMLINK "${_mui_header_lexical}")
        message(FATAL_ERROR "AHI: generated libraries/mui.h is unavailable")
    endif()
    _aros_ahi_real_path("${_gen_include_lexical}" _gen_include)
    _aros_ahi_real_path("${_mui_header_lexical}" _mui_header)
    _aros_ahi_require_make_path("generated include directory" "${_gen_include}")
    cmake_path(IS_PREFIX _build_root "${_gen_include}" NORMALIZE _gen_include_owned)
    cmake_path(IS_PREFIX _gen_include "${_mui_header}" NORMALIZE _mui_header_owned)
    if(NOT _gen_include_owned OR NOT _mui_header_owned OR
       NOT _gen_include STREQUAL "${_build_root}/GENINCDIR" OR
       NOT _mui_header STREQUAL "${_gen_include}/libraries/mui.h")
        message(FATAL_ERROR "AHI: generated MUI header escaped its expected build location")
    endif()
    set(_feature_header_lexical
        "${_mui_header_lexical}" "${_sdk_lexical}/asm/io.h")
    if(AB_MODE STREQUAL "arm" OR AB_MODE STREQUAL "aarch64")
        list(APPEND _feature_header_lexical
            "${_sdk_lexical}/proto/dma.h" "${_sdk_lexical}/proto/mbox.h")
    endif()
    set(_feature_headers "")
    foreach(_header IN LISTS _feature_header_lexical)
        if(NOT EXISTS "${_header}" OR IS_DIRECTORY "${_header}" OR IS_SYMLINK "${_header}")
            message(FATAL_ERROR "AHI: required staged feature header is unavailable: ${_header}")
        endif()
        _aros_ahi_real_path("${_header}" _header_real)
        cmake_path(IS_PREFIX _build_root "${_header_real}" NORMALIZE _header_owned)
        if(NOT _header_owned)
            message(FATAL_ERROR "AHI: staged feature header escaped the build tree")
        endif()
        list(APPEND _feature_headers "${_header_real}")
    endforeach()

    # The three link libraries this build links against, asked of their own
    # targets rather than spelled as `<build root>/liblinklibs-<mmake>.a`.
    # Carrying `conffile=` made four declarations real -- the ipv4 network
    # module, the VMM handler and two SysExplorer modules -- each of them
    # states `uselibs=amiga`, so linklibs-amiga became canonical and
    # rpi-aarch64 stopped producing a buildable graph:
    #
    #   ninja: error: 'liblinklibs-amiga.a', needed by 'SYS/Prefs/AHI',
    #          missing and no known rule to make it
    #
    # aros_linklib_archive_path carries the reasoning and the checks.
    set(_dependency_lexical "")
    foreach(_linklib IN ITEMS linklibs-amiga linklibs-libm linklibs-mui)
        aros_linklib_archive_path("${_linklib}" _linklib_archive)
        list(APPEND _dependency_lexical "${_linklib_archive}")
    endforeach()
    set(_dependency_products "")
    foreach(_dependency IN LISTS _dependency_lexical)
        cmake_path(NORMAL_PATH _dependency)
        _aros_ahi_real_path("${_dependency}" _dependency_real)
        cmake_path(IS_PREFIX _build_root "${_dependency_real}" NORMALIZE _dependency_owned)
        if(NOT _dependency_owned OR _dependency_real STREQUAL _build_root)
            message(FATAL_ERROR "AHI: dependency escaped the build tree")
        endif()
        list(APPEND _dependency_products "${_dependency_real}")
    endforeach()

    if(AB_MODE STREQUAL "x86_64")
        set(_target_triple "x86_64-unknown-aros")
        set(_compiler_triple "x86_64-unknown-elf")
        set(_elf_class "02")
        set(_machine_hex "3e00")
        set(_lld_emulation "elf_x86_64")
        set(_isa "--target=${_compiler_triple}")
    elseif(AB_MODE STREQUAL "arm")
        set(_target_triple "arm-unknown-aros")
        set(_compiler_triple "arm-none-eabi")
        set(_elf_class "01")
        set(_machine_hex "2800")
        set(_lld_emulation "armelf")
        set(_isa "--target=${_compiler_triple}" -mcpu=cortex-a7 -mfpu=neon-vfpv4
            -mfloat-abi=hard)
    else()
        set(_target_triple "aarch64-unknown-aros")
        set(_compiler_triple "aarch64-unknown-elf")
        set(_elf_class "02")
        set(_machine_hex "b700")
        set(_lld_emulation "aarch64elf")
        set(_isa "--target=${_compiler_triple}")
    endif()
    if(AROS_CROSS_TOOLCHAIN_ROOT)
        if(NOT AROS_TARGET_TRIPLE STREQUAL _target_triple)
            message(FATAL_ERROR
                "AHI: release toolchain triple ${AROS_TARGET_TRIPLE} does not match "
                "${AB_MODE} (${_target_triple})")
        endif()
        set(_compiler_triple "${_target_triple}")
        if(AB_MODE STREQUAL "arm")
            set(_isa "--target=${_compiler_triple}" -mcpu=cortex-a7
                -mfpu=neon-vfpv4 -mfloat-abi=hard)
        else()
            set(_isa "--target=${_compiler_triple}")
        endif()
    endif()
    set(_target_cflags ${_isa} -ffreestanding -fno-builtin -fno-strict-aliasing
        -fno-common -D__AROS__=1 -D__AROS_VERSION__=1 -DAMIGA=1 -D_AMIGA=1)
    # configure adds --with-os-includedir before this list.  It is set to the
    # POSIX layer below, while the generated, stdc and SDK roots retain the
    # same effective namespace order as config/specs.in.
    set(_target_cppflags ${_isa} "-I${_gen_include}" "-I${_sdk}/aros/stdc"
        "-I${_sdk}")
    set(_target_asflags ${_isa})
    set(_linkdir "${_binary_dir}/linklibs")
    # Homebrew's Clang driver delegates final links to the macOS driver.  The
    # generated adapter calls the checked AROS collector with this checked LLVM
    # linker as its backend instead.
    set(_target_ldflags ${_isa} -Wl,-r "-L${_linkdir}")
    _aros_ahi_host_triplet(_build_triplet)
    set(_stage_source "${_binary_dir}/source")
    set(_stage_build "${_binary_dir}/build")
    set(_stage_linklibs "${_binary_dir}/linklibs")
    foreach(_private IN ITEMS _stage_source _stage_build _stage_linklibs)
        _aros_ahi_real_path("${${_private}}" _private_real)
        _aros_ahi_require_make_path("${_private}" "${_private_real}")
        cmake_path(IS_PREFIX _binary_dir "${_private_real}" NORMALIZE _private_owned)
        if(NOT _private_owned OR _private_real STREQUAL _binary_dir)
            message(FATAL_ERROR "AHI: private path escaped BINARY_DIR")
        endif()
        set(${_private} "${_private_real}")
    endforeach()
    set(_cc_wrapper "${_binary_dir}/ahi-cc")
    set(_cc_wrapper_template "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/ahi-cc-wrapper.sh.in")
    if(NOT EXISTS "${_cc_wrapper_template}" OR IS_DIRECTORY "${_cc_wrapper_template}" OR
       IS_SYMLINK "${_cc_wrapper_template}")
        message(FATAL_ERROR "AHI: compiler-wrapper template is unavailable")
    endif()
    file(MAKE_DIRECTORY "${_binary_dir}")
    _aros_ahi_shell_quote("${_CMAKE_C_COMPILER}" AROS_AHI_WRAPPER_COMPILER_QUOTED)
    _aros_ahi_shell_quote("${_collect}" AROS_AHI_WRAPPER_COLLECTOR_QUOTED)
    _aros_ahi_shell_quote("${_lld}" AROS_AHI_WRAPPER_LINKER_QUOTED)
    _aros_ahi_shell_quote("${_lld_emulation}" AROS_AHI_WRAPPER_EMULATION_QUOTED)
    configure_file("${_cc_wrapper_template}" "${_cc_wrapper}" @ONLY)
    file(CHMOD "${_cc_wrapper}" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE)
    _aros_ahi_require_executable("AHI compiler wrapper" "${_cc_wrapper}" _cc_wrapper)
    set(_ar_wrapper "${_binary_dir}/ahi-ar")
    set(_ar_wrapper_template "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/ahi-ar-wrapper.sh.in")
    if(NOT EXISTS "${_ar_wrapper_template}" OR IS_DIRECTORY "${_ar_wrapper_template}" OR
       IS_SYMLINK "${_ar_wrapper_template}")
        message(FATAL_ERROR "AHI: archiver-wrapper template is unavailable")
    endif()
    _aros_ahi_shell_quote("${_CMAKE_AR}" AROS_AHI_WRAPPER_AR_QUOTED)
    configure_file("${_ar_wrapper_template}" "${_ar_wrapper}" @ONLY)
    file(CHMOD "${_ar_wrapper}" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE)
    _aros_ahi_require_executable("AHI archiver wrapper" "${_ar_wrapper}" _ar_wrapper)
    set(_flexcat_wrapper "${_binary_dir}/ahi-flexcat")
    set(_flexcat_wrapper_template
        "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/ahi-flexcat-wrapper.sh.in")
    if(NOT EXISTS "${_flexcat_wrapper_template}" OR IS_DIRECTORY "${_flexcat_wrapper_template}" OR
       IS_SYMLINK "${_flexcat_wrapper_template}")
        message(FATAL_ERROR "AHI: FlexCat-wrapper template is unavailable")
    endif()
    _aros_ahi_shell_quote("${_flexcat}" AROS_AHI_WRAPPER_FLEXCAT_QUOTED)
    configure_file("${_flexcat_wrapper_template}" "${_flexcat_wrapper}" @ONLY)
    file(CHMOD "${_flexcat_wrapper}" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE)
    _aros_ahi_require_executable("AHI FlexCat wrapper" "${_flexcat_wrapper}" _flexcat_wrapper)

    foreach(_product IN LISTS _install_outputs)
        string(SHA256 _key "${_product}")
        get_property(_owner GLOBAL PROPERTY "AROS_AHI_PRODUCT_OWNER_${_key}")
        if(_owner)
            message(FATAL_ERROR "AHI: product ${_product} is already owned by ${_owner}")
        endif()
        set_property(GLOBAL PROPERTY "AROS_AHI_PRODUCT_OWNER_${_key}" "${AB_MMAKE_ID}")
    endforeach()

    set(AHI_MMAKE_ID "${AB_MMAKE_ID}")
    set(AHI_MODE "${AB_MODE}")
    set(AHI_SOURCE_ROOT "${_source_root}")
    set(AHI_BUILD_ROOT "${_build_root}")
    set(AHI_SOURCE_DIR "${_source_dir}")
    set(AHI_SOURCE_MANIFEST "${_source_manifest}")
    set(AHI_SOURCE_MANIFEST_SHA256 "${_source_manifest_hash}")
    set(AHI_PRODUCT_MANIFEST "${_product_manifest}")
    set(AHI_PRODUCT_MANIFEST_SHA256 "${_product_manifest_hash}")
    set(AHI_BINARY_DIR "${_binary_dir}")
    set(AHI_STAGE_SOURCE "${_stage_source}")
    set(AHI_STAGE_BUILD "${_stage_build}")
    set(AHI_STAGE_LINKLIBS "${_stage_linklibs}")
    set(AHI_INSTALL_PREFIX "${_install_prefix}")
    set(AHI_HOST_SFDC "${_host_sfdc}")
    set(AHI_HOST_PERL "${_host_perl}")
    set(AHI_HOST_FLEXCAT "${_flexcat}")
    set(AHI_FLEXCAT "${_flexcat_wrapper}")
    set(AHI_MAKE "${_make}")
    set(AHI_CC "${_cc_wrapper}")
    set(AHI_COLLECT "${_collect}")
    set(AHI_AS "${_CMAKE_C_COMPILER}")
    set(AHI_AR "${_ar_wrapper}")
    set(AHI_RANLIB "${_CMAKE_RANLIB}")
    set(AHI_OBJCOPY "${_CMAKE_OBJCOPY}")
    set(AHI_STRIP "${_CMAKE_STRIP}")
    set(AHI_LLD "${_lld}")
    set(AHI_SDK_INCLUDE "${_sdk}")
    set(AHI_GEN_INCLUDE "${_gen_include}")
    set(AHI_FEATURE_HEADERS "${_feature_headers}")
    set(AHI_BUILD_TRIPLET "${_build_triplet}")
    set(AHI_TARGET_TRIPLE "${_target_triple}")
    set(AHI_ELF_CLASS "${_elf_class}")
    set(AHI_ELF_MACHINE_HEX "${_machine_hex}")
    set(AHI_TARGET_CFLAGS "${_target_cflags}")
    set(AHI_TARGET_CPPFLAGS "${_target_cppflags}")
    set(AHI_TARGET_ASFLAGS "${_target_asflags}")
    set(AHI_TARGET_LDFLAGS "${_target_ldflags}")
    set(AHI_INPUT_RELATIVE "${_input_relative}")
    set(AHI_INPUT_SHA256 "${_input_hashes}")
    set(AHI_PRODUCT_RELATIVE "${_product_relative}")
    set(AHI_PRODUCT_KINDS "${_product_kinds}")
    set(AHI_INSTALL_PRODUCTS "${_install_outputs}")
    set(AHI_DEPENDENCY_PRODUCTS "${_dependency_products}")
    set(_contract "${CMAKE_CURRENT_BINARY_DIR}/.aros-${AB_MMAKE_ID}-ahi-contract.cmake")
    set(_content "")
    foreach(_var IN ITEMS
            AHI_MMAKE_ID AHI_MODE AHI_SOURCE_ROOT AHI_BUILD_ROOT AHI_SOURCE_DIR
            AHI_SOURCE_MANIFEST AHI_SOURCE_MANIFEST_SHA256
            AHI_PRODUCT_MANIFEST AHI_PRODUCT_MANIFEST_SHA256 AHI_BINARY_DIR
            AHI_STAGE_SOURCE AHI_STAGE_BUILD AHI_STAGE_LINKLIBS AHI_INSTALL_PREFIX
            AHI_HOST_SFDC AHI_HOST_PERL AHI_HOST_FLEXCAT AHI_FLEXCAT AHI_MAKE
            AHI_CC AHI_COLLECT AHI_AS AHI_AR AHI_RANLIB AHI_OBJCOPY AHI_STRIP
            AHI_LLD AHI_SDK_INCLUDE
            AHI_GEN_INCLUDE AHI_FEATURE_HEADERS
            AHI_BUILD_TRIPLET AHI_TARGET_TRIPLE AHI_ELF_CLASS AHI_ELF_MACHINE_HEX
            AHI_TARGET_CFLAGS AHI_TARGET_CPPFLAGS AHI_TARGET_ASFLAGS AHI_TARGET_LDFLAGS
            AHI_INPUT_RELATIVE AHI_INPUT_SHA256 AHI_PRODUCT_RELATIVE AHI_PRODUCT_KINDS
            AHI_INSTALL_PRODUCTS AHI_DEPENDENCY_PRODUCTS)
        # List members were validated before collection.  CMake serializes a
        # list with semicolons, which are not unsafe here; applying the scalar
        # validator to that representation would reject every valid contract.
        if(NOT _var MATCHES
                "^(AHI_TARGET_CFLAGS|AHI_TARGET_CPPFLAGS|AHI_TARGET_ASFLAGS|AHI_TARGET_LDFLAGS|AHI_INPUT_RELATIVE|AHI_INPUT_SHA256|AHI_PRODUCT_RELATIVE|AHI_PRODUCT_KINDS|AHI_INSTALL_PRODUCTS|AHI_DEPENDENCY_PRODUCTS|AHI_FEATURE_HEADERS)$")
            _aros_ahi_safe_value("${_var}" "${${_var}}")
        endif()
        string(APPEND _content "set(${_var} [==[${${_var}}]==])\n")
    endforeach()
    file(GENERATE OUTPUT "${_contract}" CONTENT "${_content}")

    add_custom_command(
        OUTPUT ${_install_outputs_lexical}
        COMMAND "${_ahi_runner}" --contract "${_contract}"
        DEPENDS "${_ahi_runner}" "${_contract}" "${_source_manifest_lexical}"
            "${_product_manifest_lexical}" "${_HOST_SFDC_lexical}" "${_flexcat_lexical}"
            "${_cc_wrapper}" "${_cc_wrapper_template}"
            "${_ar_wrapper}" "${_ar_wrapper_template}"
            "${_flexcat_wrapper}" "${_flexcat_wrapper_template}" "${_collect}" "${_lld}"
            ${_dependency_lexical} ${_feature_header_lexical} ${_input_dependencies}
        COMMENT "Building closed AHI ${AB_MODE} capability"
        VERBATIM
        COMMAND_EXPAND_LISTS)
    add_custom_target("${AB_MMAKE_ID}" DEPENDS ${_install_outputs_lexical})
    set(AROS_AHI_INSTALL_PRODUCTS "${_install_outputs_lexical}" PARENT_SCOPE)
endfunction()
