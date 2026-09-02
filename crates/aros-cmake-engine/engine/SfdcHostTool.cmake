include_guard(GLOBAL)

include(CMakeParseArguments)
include("${CMAKE_CURRENT_LIST_DIR}/Executable.cmake")

# Closed host-side builder for the historic Perl SFD compiler.  This is kept
# separate from HostTools.cmake until a translated declaration consumes it:
# unlike the C host tools there is no target compilation, and the executable
# is an exact concatenation of a small, audited Perl source closure.
set(_AROS_SFDC_VERSION "1.3")
set(_AROS_SFDC_DATE "2004-11-12")

# Resolve existing components physically and retain a non-existing tail.  A
# lexical NORMALIZE alone is insufficient on macOS, where /tmp is normally a
# symlink to /private/tmp.
function(_aros_sfdc_real_path path output)
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

function(_aros_sfdc_validate_contract_value name value)
    if("${value}" MATCHES "[;\"$\\\\\r\n]" OR
       "${value}" MATCHES "==\\]")
        message(FATAL_ERROR "host-sfdc: unsafe ${name} '${value}'")
    endif()
endfunction()

# aros_build_host_sfdc(PERL <absolute-perl-interpreter>)
#
# Produces only ${CMAKE_BINARY_DIR}/hosttools/sfdc and exports that path as
# AROS_HOST_SFDC to the caller.  The optional AROS_SFDC_SOURCE_ROOT variable is
# intentionally useful only for focused fixtures; production defaults to the
# top-level source tree and still requires the explicit checked input manifest.
function(aros_build_host_sfdc)
    set(one_value_args PERL)
    cmake_parse_arguments(PARSE_ARGV 0 SFDC "" "${one_value_args}" "")
    if(SFDC_UNPARSED_ARGUMENTS OR SFDC_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "aros_build_host_sfdc received malformed arguments")
    endif()
    if(NOT SFDC_PERL)
        message(FATAL_ERROR
            "aros_build_host_sfdc requires PERL; host discovery via PATH is forbidden")
    endif()
    if(TARGET host-sfdc)
        message(FATAL_ERROR "host-sfdc was already declared")
    endif()

    if(DEFINED AROS_SFDC_SOURCE_ROOT)
        set(_source_root_raw "${AROS_SFDC_SOURCE_ROOT}")
    else()
        set(_source_root_raw "${CMAKE_SOURCE_DIR}")
    endif()
    set(_build_root_raw "${CMAKE_BINARY_DIR}")
    foreach(_pair IN ITEMS
            "source root|${_source_root_raw}"
            "build root|${_build_root_raw}"
            "Perl interpreter|${SFDC_PERL}")
        string(REPLACE "|" ";" _parts "${_pair}")
        list(GET _parts 0 _name)
        list(GET _parts 1 _value)
        _aros_sfdc_validate_contract_value("${_name}" "${_value}")
    endforeach()

    cmake_path(ABSOLUTE_PATH _source_root_raw NORMALIZE
        OUTPUT_VARIABLE _source_root_lexical)
    cmake_path(ABSOLUTE_PATH _build_root_raw NORMALIZE
        OUTPUT_VARIABLE _build_root_lexical)
    set(_perl_raw "${SFDC_PERL}")
    if(NOT IS_ABSOLUTE "${_perl_raw}")
        message(FATAL_ERROR "host-sfdc: PERL must be an absolute path")
    endif()
    cmake_path(NORMAL_PATH _perl_raw OUTPUT_VARIABLE _perl_lexical)

    if(NOT EXISTS "${_source_root_lexical}" OR
       NOT IS_DIRECTORY "${_source_root_lexical}")
        message(FATAL_ERROR "host-sfdc: source root is unavailable")
    endif()
    if(NOT EXISTS "${_build_root_lexical}" OR
       NOT IS_DIRECTORY "${_build_root_lexical}")
        message(FATAL_ERROR "host-sfdc: build root is unavailable")
    endif()
    aros_path_is_executable("${_perl_lexical}" _perl_executable)
    if(NOT _perl_executable)
        message(FATAL_ERROR "host-sfdc: PERL is not a regular executable")
    endif()

    _aros_sfdc_real_path("${_source_root_lexical}" _source_root)
    _aros_sfdc_real_path("${_build_root_lexical}" _build_root)
    _aros_sfdc_real_path("${_perl_lexical}" _perl)
    if(_perl MATCHES "[[:space:]]")
        message(FATAL_ERROR
            "host-sfdc: PERL path cannot contain whitespace for a safe shebang")
    endif()

    set(_source_dir_lexical "${_source_root_lexical}/tools/sfdc")
    set(_manifest_lexical "${_source_dir_lexical}/sfdc-host.inputs")
    set(_binary_dir_lexical "${_build_root_lexical}/gen/hosttools/sfdc")
    set(_output_lexical "${_build_root_lexical}/hosttools/sfdc")
    foreach(_path IN ITEMS _source_dir_lexical _manifest_lexical
            _binary_dir_lexical _output_lexical)
        cmake_path(NORMAL_PATH ${_path})
    endforeach()
    if(NOT EXISTS "${_source_dir_lexical}" OR
       NOT IS_DIRECTORY "${_source_dir_lexical}")
        message(FATAL_ERROR "host-sfdc: audited source directory is unavailable")
    endif()
    if(NOT EXISTS "${_manifest_lexical}" OR IS_DIRECTORY "${_manifest_lexical}" OR
       IS_SYMLINK "${_manifest_lexical}")
        message(FATAL_ERROR "host-sfdc: audited input manifest is unavailable")
    endif()

    _aros_sfdc_real_path("${_source_dir_lexical}" _source_dir)
    _aros_sfdc_real_path("${_manifest_lexical}" _manifest)
    _aros_sfdc_real_path("${_binary_dir_lexical}" _binary_dir)
    _aros_sfdc_real_path("${_output_lexical}" _output)
    cmake_path(IS_PREFIX _source_root "${_source_dir}" NORMALIZE _source_owned)
    cmake_path(IS_PREFIX _source_dir "${_manifest}" NORMALIZE _manifest_owned)
    cmake_path(IS_PREFIX _build_root "${_binary_dir}" NORMALIZE _binary_owned)
    cmake_path(IS_PREFIX _build_root "${_output}" NORMALIZE _output_owned)
    if(NOT _source_owned OR _source_dir STREQUAL _source_root OR
       NOT _manifest_owned OR NOT _binary_owned OR _binary_dir STREQUAL _build_root OR
       NOT _output_owned OR _output STREQUAL _build_root)
        message(FATAL_ERROR "host-sfdc: audited paths escape their owning tree")
    endif()

    file(SHA256 "${_manifest}" _actual_manifest_sha256)
    file(STRINGS "${_manifest}" _manifest_lines ENCODING UTF-8)
    if(NOT _manifest_lines)
        message(FATAL_ERROR "host-sfdc: input manifest is empty")
    endif()

    set(_input_files "")
    set(_manifest_paths "")
    set(_input_hashes "")
    set(_output_content "#!${_perl} -w\n")
    foreach(_line IN LISTS _manifest_lines)
        if(NOT _line MATCHES "^([A-Za-z0-9_.+/-]+)$")
            message(FATAL_ERROR "host-sfdc: malformed input-manifest line '${_line}'")
        endif()
        set(_relative "${CMAKE_MATCH_1}")
        if(IS_ABSOLUTE "${_relative}" OR
           _relative MATCHES "(^|/)[.][.]?(/|$)" OR
           _relative IN_LIST _manifest_paths)
            message(FATAL_ERROR "host-sfdc: unsafe input-manifest line '${_line}'")
        endif()
        list(APPEND _manifest_paths "${_relative}")
        set(_input_lexical "${_source_dir_lexical}/${_relative}")
        cmake_path(NORMAL_PATH _input_lexical)
        if(NOT EXISTS "${_input_lexical}" OR IS_DIRECTORY "${_input_lexical}" OR
           IS_SYMLINK "${_input_lexical}")
            message(FATAL_ERROR "host-sfdc: input ${_relative} is missing or symlinked")
        endif()
        _aros_sfdc_real_path("${_input_lexical}" _input)
        cmake_path(IS_PREFIX _source_dir "${_input}" NORMALIZE _input_owned)
        if(NOT _input_owned)
            message(FATAL_ERROR "host-sfdc: input ${_relative} escaped the source tree")
        endif()
        file(SHA256 "${_input}" _actual_input_sha256)
        list(APPEND _input_hashes "${_actual_input_sha256}")
        file(READ "${_input}" _input_content)
        if(_relative STREQUAL "main.pl")
            string(REGEX REPLACE "^#![^\n]*\n" "" _input_content "${_input_content}")
        endif()
        string(REPLACE "SFDC_VERSION" "${_AROS_SFDC_VERSION}"
            _input_content "${_input_content}")
        string(REPLACE "SFDC_DATE" "${_AROS_SFDC_DATE}"
            _input_content "${_input_content}")
        string(APPEND _output_content "${_input_content}")
        list(APPEND _input_files "${_input_lexical}")
    endforeach()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
        "${_manifest_lexical}" ${_input_files})
    string(SHA256 _output_sha256 "${_output_content}")

    string(SHA256 _product_key "${_output}")
    get_property(_previous_owner GLOBAL PROPERTY
        "AROS_SFDC_PRODUCT_OWNER_${_product_key}")
    if(_previous_owner)
        message(FATAL_ERROR "host-sfdc output is already owned by ${_previous_owner}")
    endif()
    set_property(GLOBAL PROPERTY "AROS_SFDC_PRODUCT_OWNER_${_product_key}" "host-sfdc")

    set(_contract "${CMAKE_CURRENT_BINARY_DIR}/.aros-host-sfdc-contract.cmake")
    set(_contract_content "")
    foreach(_pair IN ITEMS
            "SFDC_MMAKE_ID|host-sfdc"
            "SFDC_SOURCE_ROOT|${_source_root}"
            "SFDC_BUILD_ROOT|${_build_root}"
            "SFDC_SOURCE_DIR|${_source_dir}"
            "SFDC_BINARY_DIR|${_binary_dir}"
            "SFDC_OUTPUT|${_output}"
            "SFDC_INPUT_MANIFEST|${_manifest}"
            "SFDC_INPUT_MANIFEST_SHA256|${_actual_manifest_sha256}"
            "SFDC_PERL|${_perl}"
            "SFDC_VERSION|${_AROS_SFDC_VERSION}"
            "SFDC_DATE|${_AROS_SFDC_DATE}"
            "SFDC_OUTPUT_SHA256|${_output_sha256}")
        string(FIND "${_pair}" "|" _separator)
        string(SUBSTRING "${_pair}" 0 ${_separator} _name)
        math(EXPR _value_start "${_separator} + 1")
        string(SUBSTRING "${_pair}" ${_value_start} -1 _value)
        string(APPEND _contract_content "set(${_name} [==[${_value}]==])\n")
    endforeach()
    string(APPEND _contract_content "set(SFDC_INPUT_RELATIVE)\nset(SFDC_INPUT_SHA256)\n")
    foreach(_relative IN LISTS _manifest_paths)
        string(APPEND _contract_content
            "list(APPEND SFDC_INPUT_RELATIVE [==[${_relative}]==])\n")
    endforeach()
    foreach(_digest IN LISTS _input_hashes)
        string(APPEND _contract_content
            "list(APPEND SFDC_INPUT_SHA256 [==[${_digest}]==])\n")
    endforeach()
    file(GENERATE OUTPUT "${_contract}" CONTENT "${_contract_content}")

    set(_runner "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSfdcHostTool.cmake")
    add_custom_command(
        OUTPUT "${_output_lexical}"
        COMMAND "${CMAKE_COMMAND}" "-DCONTRACT=${_contract}" -P "${_runner}"
        DEPENDS "${_runner}" "${_contract}" "${_manifest_lexical}"
            "${_perl_lexical}" ${_input_files}
        COMMENT "Building closed host tool sfdc"
        VERBATIM
        COMMAND_EXPAND_LISTS)
    add_custom_target(host-sfdc DEPENDS "${_output_lexical}")
    set(AROS_HOST_SFDC "${_output_lexical}" PARENT_SCOPE)
endfunction()
