# Output-owning CMake counterpart for the narrowly modelled handwritten
# GENMODULE `.includes-generated` recipe.

include_guard(GLOBAL)
include(CMakeParseArguments)

function(_aros_genmodule_header_validate_relative value label allow_empty)
    if("${value}" STREQUAL "" AND allow_empty)
        return()
    endif()
    if("${value}" STREQUAL "" OR IS_ABSOLUTE "${value}" OR
       "${value}" MATCHES "[\\;:$<>|\"'`\n\r]")
        message(FATAL_ERROR
            "aros_genmodule_header_stamp: ${label} is not a safe relative path: '${value}'")
    endif()
    string(REPLACE "\\" "/" _normal "${value}")
    if(NOT "${_normal}" STREQUAL "${value}" OR
       "${_normal}" MATCHES "(^|/)[.]($|/)" OR
       "${_normal}" MATCHES "(^|/)[.][.]($|/)")
        message(FATAL_ERROR
            "aros_genmodule_header_stamp: ${label} contains traversal: '${value}'")
    endif()
    string(REPLACE "/" ";" _components "${value}")
    foreach(_component IN LISTS _components)
        if(NOT "${_component}" MATCHES "^[A-Za-z0-9_+.-]+$" OR
           "${_component}" STREQUAL "." OR "${_component}" STREQUAL "..")
            message(FATAL_ERROR
                "aros_genmodule_header_stamp: ${label} has an unsafe component '${_component}'")
        endif()
    endforeach()
endfunction()

function(_aros_genmodule_header_check_output_root root binary_root owner)
    set(_root "${root}")
    set(_binary_root "${binary_root}")
    cmake_path(NORMAL_PATH _root OUTPUT_VARIABLE _root)
    cmake_path(NORMAL_PATH _binary_root OUTPUT_VARIABLE _binary_root)
    set(_binary_root_path "${_binary_root}")
    cmake_path(IS_PREFIX _binary_root_path "${_root}" NORMALIZE _lexically_inside)
    if(NOT _lexically_inside AND NOT "${_root}" STREQUAL "${_binary_root}")
        message(FATAL_ERROR
            "${owner}: configured output root '${_root}' is outside CMAKE_BINARY_DIR")
    endif()
    if(IS_SYMLINK "${_binary_root}" OR
       NOT EXISTS "${_binary_root}" OR NOT IS_DIRECTORY "${_binary_root}")
        message(FATAL_ERROR
            "${owner}: CMAKE_BINARY_DIR '${_binary_root}' is not a real directory")
    endif()
    file(REAL_PATH "${_binary_root}" _binary_root_real)
    file(RELATIVE_PATH _tail "${_binary_root}" "${_root}")
    string(REPLACE "/" ";" _components "${_tail}")
    set(_cursor "${_binary_root}")
    foreach(_component IN LISTS _components)
        if("${_component}" STREQUAL "" OR "${_component}" STREQUAL ".")
            continue()
        endif()
        set(_cursor "${_cursor}/${_component}")
        if(IS_SYMLINK "${_cursor}")
            message(FATAL_ERROR
                "${owner}: configured output root path crosses symlink '${_cursor}'")
        endif()
        if(EXISTS "${_cursor}")
            if(NOT IS_DIRECTORY "${_cursor}")
                message(FATAL_ERROR
                    "${owner}: configured output root path crosses non-directory '${_cursor}'")
            endif()
            file(REAL_PATH "${_cursor}" _cursor_real)
            set(_binary_root_real_path "${_binary_root_real}")
            cmake_path(IS_PREFIX _binary_root_real_path "${_cursor_real}"
                NORMALIZE _physically_inside)
            if(NOT _physically_inside AND
               NOT "${_cursor_real}" STREQUAL "${_binary_root_real}")
                message(FATAL_ERROR
                    "${owner}: configured output root resolves outside CMAKE_BINARY_DIR: '${_cursor_real}'")
            endif()
        endif()
    endforeach()
    if(NOT EXISTS "${_root}" OR NOT IS_DIRECTORY "${_root}")
        message(FATAL_ERROR
            "${owner}: configured output root '${_root}' is not a directory")
    endif()
    file(REAL_PATH "${_root}" _root_real)
    set(_binary_root_real_path "${_binary_root_real}")
    cmake_path(IS_PREFIX _binary_root_real_path "${_root_real}"
        NORMALIZE _physically_inside)
    if(NOT _physically_inside AND NOT "${_root_real}" STREQUAL "${_binary_root_real}")
        message(FATAL_ERROR
            "${owner}: configured output root resolves outside CMAKE_BINARY_DIR: '${_root_real}'")
    endif()
endfunction()

# Check one generated path against its configured root. Existing symlinks,
# non-directory parents, and physical escapes are rejected before Ninja claims
# the output; not-yet-created descendants are checked lexically and again on
# the next configure after they exist.
function(_aros_genmodule_header_check_output output root owner)
    set(_root "${root}")
    set(_output "${output}")
    cmake_path(NORMAL_PATH _root OUTPUT_VARIABLE _root)
    cmake_path(NORMAL_PATH _output OUTPUT_VARIABLE _output)
    set(_root_path "${_root}")
    cmake_path(IS_PREFIX _root_path "${_output}" NORMALIZE _inside)
    if(NOT _inside)
        message(FATAL_ERROR
            "${owner}: generated output '${_output}' escaped configured root '${_root}'")
    endif()
    if(IS_SYMLINK "${_root}")
        message(FATAL_ERROR "${owner}: configured output root '${_root}' is a symlink")
    endif()
    if(NOT EXISTS "${_root}" OR NOT IS_DIRECTORY "${_root}")
        message(FATAL_ERROR "${owner}: configured output root '${_root}' is not a directory")
    endif()
    file(REAL_PATH "${_root}" _root_real)
    file(RELATIVE_PATH _tail "${_root}" "${_output}")
    string(REPLACE "/" ";" _parts "${_tail}")
    set(_cursor "${_root}")
    list(LENGTH _parts _part_count)
    set(_part_index 0)
    foreach(_part IN LISTS _parts)
        if("${_part}" STREQUAL "")
            continue()
        endif()
        math(EXPR _part_index "${_part_index} + 1")
        set(_cursor "${_cursor}/${_part}")
        if(IS_SYMLINK "${_cursor}")
            message(FATAL_ERROR
                "${owner}: generated output path crosses symlink '${_cursor}'")
        endif()
        if(EXISTS "${_cursor}")
            if(_part_index EQUAL _part_count AND IS_DIRECTORY "${_cursor}")
                message(FATAL_ERROR
                    "${owner}: generated file output '${_cursor}' is already a directory")
            endif()
            file(REAL_PATH "${_cursor}" _cursor_real)
            set(_root_real_path "${_root_real}")
            cmake_path(IS_PREFIX _root_real_path "${_cursor_real}" NORMALIZE _physical_inside)
            if(NOT _physical_inside AND NOT "${_cursor_real}" STREQUAL "${_root_real}")
                message(FATAL_ERROR
                    "${owner}: generated output path resolves outside '${_root_real}'")
            endif()
        else()
            break()
        endif()
    endforeach()
endfunction()

function(_aros_genmodule_header_validate_config config module modtype include_name owner)
    file(STRINGS "${config}" _lines)
    set(_section "")
    set(_config_seen FALSE)
    set(_includename "")
    set(_saw_includes FALSE)
    set(_saw_noincludes FALSE)
    set(_allowed_sections config cdef cdefprivate functionlist cfunctionlist)
    set(_config_keys
        basename libbase libbasetype libbasetypeextern version date copyright libcall
        forcebase superclass superclass_field residentpri options sysbase_field
        seglist_field rootbase_field classptr_field classptr_var classid classdatatype
        beginio_func abortio_func dispatcher initpri type addromtag oopbase_field rellib
        interfaceid interfacename methodstub methodbase attributebase handler_func includename)
    set(_options
        noautolib noexpunge noresident peropenerbase pertaskbase includes noincludes nostubs
        autoinit noautoinit resautoinit noinittable noresstruct nofunctable noopenclose
        selfinit rellinklib noclassquery)

    foreach(_raw IN LISTS _lines)
        string(STRIP "${_raw}" _line)
        if("${_line}" STREQUAL "" OR "${_line}" MATCHES "^#[^#]")
            continue()
        endif()
        if("${_line}" MATCHES "^##begin[ \t]+([^ \t]+).*$")
            set(_next_section "${CMAKE_MATCH_1}")
            if(NOT "${_section}" STREQUAL "")
                message(FATAL_ERROR "${owner}: nested genmodule config sections are unsupported")
            endif()
            if(NOT _next_section IN_LIST _allowed_sections)
                # Defined below; the explicit branch keeps output-bearing
                # interface/class sections visibly unsupported.
                if("${_next_section}" STREQUAL "interface" OR
                   "${_next_section}" STREQUAL "class")
                    message(FATAL_ERROR
                        "${owner}: genmodule ${_next_section} section adds headers outside the declared output set")
                endif()
                message(FATAL_ERROR
                    "${owner}: unsupported genmodule config section '${_next_section}'")
            endif()
            if("${_next_section}" STREQUAL "config")
                if(_config_seen)
                    message(FATAL_ERROR "${owner}: genmodule config has multiple config sections")
                endif()
                set(_config_seen TRUE)
            endif()
            set(_section "${_next_section}")
            continue()
        endif()
        if("${_line}" MATCHES "^##end[ \t]+([^ \t]+).*$")
            if(NOT "${CMAKE_MATCH_1}" STREQUAL "${_section}")
                message(FATAL_ERROR "${owner}: mismatched genmodule config section end")
            endif()
            set(_section "")
            continue()
        endif()
        if("${_line}" MATCHES "^##")
            message(FATAL_ERROR "${owner}: malformed genmodule config section marker '${_line}'")
        endif()
        if("${_section}" STREQUAL "config")
            if(NOT "${_line}" MATCHES "^([A-Za-z_][A-Za-z0-9_]*)[ \t]+(.+)$")
                message(FATAL_ERROR "${owner}: malformed genmodule config option '${_line}'")
            endif()
            set(_key "${CMAKE_MATCH_1}")
            set(_value "${CMAKE_MATCH_2}")
            if(NOT _key IN_LIST _config_keys)
                message(FATAL_ERROR "${owner}: unsupported genmodule config option '${_key}'")
            endif()
            if("${_key}" STREQUAL "includename")
                if(NOT "${_includename}" STREQUAL "")
                    message(FATAL_ERROR "${owner}: duplicate genmodule includename")
                endif()
                if(NOT "${_value}" MATCHES "^[A-Za-z0-9_-]+$")
                    message(FATAL_ERROR "${owner}: genmodule includename '${_value}' is unsafe")
                endif()
                set(_includename "${_value}")
            elseif("${_key}" STREQUAL "options")
                string(REPLACE "," ";" _option_tokens "${_value}")
                string(REPLACE " " ";" _option_tokens "${_option_tokens}")
                string(REPLACE "\t" ";" _option_tokens "${_option_tokens}")
                foreach(_option IN LISTS _option_tokens)
                    if("${_option}" STREQUAL "")
                        continue()
                    endif()
                    if(NOT _option IN_LIST _options)
                        message(FATAL_ERROR "${owner}: unsupported genmodule option '${_option}'")
                    endif()
                    if("${_option}" STREQUAL "includes")
                        set(_saw_includes TRUE)
                    elseif("${_option}" STREQUAL "noincludes")
                        set(_saw_noincludes TRUE)
                    endif()
                endforeach()
            endif()
        elseif("${_section}" STREQUAL "")
            message(FATAL_ERROR "${owner}: genmodule config content is outside a supported section")
        endif()
    endforeach()

    if(NOT _config_seen OR NOT "${_section}" STREQUAL "")
        message(FATAL_ERROR "${owner}: genmodule config is unclosed or has no config section")
    endif()
    if(_saw_includes AND _saw_noincludes)
        message(FATAL_ERROR "${owner}: genmodule config combines includes and noincludes")
    endif()
    if(_saw_noincludes)
        message(FATAL_ERROR "${owner}: writeincludes is disabled by genmodule config")
    endif()
    if(NOT _saw_includes AND NOT modtype STREQUAL "resource" AND NOT modtype STREQUAL "library")
        message(FATAL_ERROR
            "${owner}: non-library/resource config needs explicit options includes")
    endif()
    if("${_includename}" STREQUAL "")
        set(_includename "${module}")
    endif()
    if(NOT "${_includename}" STREQUAL "${include_name}")
        message(FATAL_ERROR
            "${owner}: declared include name '${include_name}' differs from config-derived '${_includename}'")
    endif()
endfunction()

# aros_genmodule_header_stamp(
#     NAME <real-owner-target> DECLARING_DIR <source-relative-directory>
#     CONFIG <source-relative-config> MODULE <module> MODTYPE <type>
#     INCLUDE_NAME <config-derived-name>)
function(aros_genmodule_header_stamp)
    set(oneValueArgs NAME DECLARING_DIR CONFIG MODULE MODTYPE INCLUDE_NAME LAYOUT)
    cmake_parse_arguments(GHS "" "${oneValueArgs}" "" ${ARGN})
    if(GHS_UNPARSED_ARGUMENTS OR GHS_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_genmodule_header_stamp: malformed arguments: "
            "${GHS_UNPARSED_ARGUMENTS}${GHS_KEYWORDS_MISSING_VALUES}")
    endif()
    foreach(_required NAME CONFIG MODULE MODTYPE INCLUDE_NAME LAYOUT)
        if(NOT DEFINED GHS_${_required} OR "${GHS_${_required}}" STREQUAL "")
            message(FATAL_ERROR "aros_genmodule_header_stamp: ${_required} is required")
        endif()
    endforeach()
    if(NOT DEFINED GHS_DECLARING_DIR)
        set(GHS_DECLARING_DIR "")
    endif()
    if(TARGET "${GHS_NAME}")
        message(FATAL_ERROR "aros_genmodule_header_stamp: target '${GHS_NAME}' already exists")
    endif()
    if(NOT "${GHS_NAME}" MATCHES "^[A-Za-z0-9_][A-Za-z0-9_.+-]*$")
        message(FATAL_ERROR "aros_genmodule_header_stamp: unsafe target name '${GHS_NAME}'")
    endif()
    foreach(_root_var AROS_SOURCE_DIR AROS_BUILD_DIR AROS_GENINC_DIR AROS_SDK_INCLUDE_DIR)
        if(NOT DEFINED ${_root_var} OR NOT IS_ABSOLUTE "${${_root_var}}")
            message(FATAL_ERROR "aros_genmodule_header_stamp: ${_root_var} must be absolute")
        endif()
    endforeach()
    if(NOT AROS_HOST_GENMODULE OR NOT IS_ABSOLUTE "${AROS_HOST_GENMODULE}")
        message(FATAL_ERROR
            "aros_genmodule_header_stamp: AROS_HOST_GENMODULE must be an absolute executable path")
    endif()
    if(IS_SYMLINK "${AROS_HOST_GENMODULE}")
        message(FATAL_ERROR "aros_genmodule_header_stamp: host genmodule tool is a symlink")
    endif()
    if(NOT "${GHS_MODULE}" MATCHES "^[A-Za-z0-9_-]+$")
        message(FATAL_ERROR "aros_genmodule_header_stamp: unsafe module '${GHS_MODULE}'")
    endif()
    if(NOT "${GHS_INCLUDE_NAME}" MATCHES "^[A-Za-z0-9_-]+$")
        message(FATAL_ERROR "aros_genmodule_header_stamp: unsafe include name '${GHS_INCLUDE_NAME}'")
    endif()
    set(_valid_modtypes class library mcc mui mcp device resource gadget image datatype
        usbclass btclass hidd handler hook)
    if(NOT GHS_MODTYPE IN_LIST _valid_modtypes)
        message(FATAL_ERROR "aros_genmodule_header_stamp: unsupported modtype '${GHS_MODTYPE}'")
    endif()
    set(_valid_layouts Full PrivateOnly PublicOnly)
    if(NOT GHS_LAYOUT IN_LIST _valid_layouts)
        message(FATAL_ERROR "aros_genmodule_header_stamp: unsupported layout '${GHS_LAYOUT}'")
    endif()
    _aros_genmodule_header_validate_relative(
        "${GHS_DECLARING_DIR}" "DECLARING_DIR" TRUE)
    _aros_genmodule_header_validate_relative("${GHS_CONFIG}" "CONFIG" FALSE)
    cmake_path(GET GHS_CONFIG PARENT_PATH _config_dir)
    cmake_path(GET GHS_CONFIG FILENAME _config_name)
    if(NOT "${_config_dir}" STREQUAL "${GHS_DECLARING_DIR}" OR
       NOT "${_config_name}" MATCHES "^[A-Za-z0-9_+-]+\\.conf$")
        message(FATAL_ERROR
            "aros_genmodule_header_stamp: CONFIG must be one .conf file in DECLARING_DIR")
    endif()

    string(REPLACE "\\" "/" _config_rel "${GHS_CONFIG}")
    set(_config "${AROS_SOURCE_DIR}/${_config_rel}")
    cmake_path(NORMAL_PATH _config OUTPUT_VARIABLE _config)
    if(NOT EXISTS "${_config}" OR IS_DIRECTORY "${_config}" OR IS_SYMLINK "${_config}")
        message(FATAL_ERROR
            "aros_genmodule_header_stamp: config is missing or is not a regular non-symlink file: ${_config}")
    endif()
    file(REAL_PATH "${AROS_SOURCE_DIR}" _source_real)
    file(REAL_PATH "${_config}" _config_real)
    set(_source_real_path "${_source_real}")
    cmake_path(IS_PREFIX _source_real_path "${_config_real}" NORMALIZE _config_inside)
    if(NOT _config_inside)
        message(FATAL_ERROR "aros_genmodule_header_stamp: config resolves outside source tree")
    endif()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${_config_real}")
    _aros_genmodule_header_validate_config(
        "${_config_real}" "${GHS_MODULE}" "${GHS_MODTYPE}" "${GHS_INCLUDE_NAME}" "${GHS_NAME}")

    cmake_path(NORMAL_PATH AROS_BUILD_DIR OUTPUT_VARIABLE _build_root)
    cmake_path(NORMAL_PATH AROS_GENINC_DIR OUTPUT_VARIABLE _geninc_root)
    cmake_path(NORMAL_PATH AROS_SDK_INCLUDE_DIR OUTPUT_VARIABLE _sdk_root)
    set(_binary_root "${CMAKE_BINARY_DIR}")
    cmake_path(NORMAL_PATH _binary_root OUTPUT_VARIABLE _binary_root)
    _aros_genmodule_header_check_output_root(
        "${_build_root}" "${_binary_root}" "${GHS_NAME}")
    _aros_genmodule_header_check_output_root(
        "${_geninc_root}" "${_binary_root}" "${GHS_NAME}")
    _aros_genmodule_header_check_output_root(
        "${_sdk_root}" "${_binary_root}" "${GHS_NAME}")
    set(_include_roots "${_geninc_root}" "${_sdk_root}")
    if(GHS_LAYOUT STREQUAL "Full" OR GHS_LAYOUT STREQUAL "PrivateOnly")
        set(_private_probe "${_build_root}/gen")
        if(NOT "${GHS_DECLARING_DIR}" STREQUAL "")
            string(APPEND _private_probe "/${GHS_DECLARING_DIR}")
        endif()
        string(APPEND _private_probe "/include")
        list(APPEND _include_roots "${_private_probe}")
    endif()
    list(LENGTH _include_roots _include_root_count)
    if(_include_root_count GREATER 1)
        math(EXPR _last_root_index "${_include_root_count} - 1")
        foreach(_left_index RANGE 0 ${_last_root_index})
            math(EXPR _right_start "${_left_index} + 1")
            if(_right_start GREATER _last_root_index)
                continue()
            endif()
            foreach(_right_index RANGE ${_right_start} ${_last_root_index})
                list(GET _include_roots ${_left_index} _left_root)
                list(GET _include_roots ${_right_index} _right_root)
                set(_left_root_path "${_left_root}")
                set(_right_root_path "${_right_root}")
                cmake_path(IS_PREFIX _left_root_path "${_right_root}"
                    NORMALIZE _left_contains)
                cmake_path(IS_PREFIX _right_root_path "${_left_root}"
                    NORMALIZE _right_contains)
                if(_left_contains OR _right_contains OR
                   "${_left_root}" STREQUAL "${_right_root}")
                    message(FATAL_ERROR
                        "${GHS_NAME}: configured include roots overlap or alias: '${_left_root}' and '${_right_root}'")
                endif()
            endforeach()
        endforeach()
    endif()

    if("${GHS_DECLARING_DIR}" STREQUAL "")
        set(_private_module_root "${_build_root}/gen")
    else()
        set(_private_module_root "${_build_root}/gen/${GHS_DECLARING_DIR}")
    endif()
    set(_private_include "${_private_module_root}/include")
    set(_stamp "${_private_module_root}/.includes-generated")
    set(_header_rel
        "clib/${GHS_INCLUDE_NAME}_protos.h"
        "inline/${GHS_INCLUDE_NAME}.h"
        "defines/${GHS_INCLUDE_NAME}.h"
        "defines/${GHS_INCLUDE_NAME}_LVO.h"
        "proto/${GHS_INCLUDE_NAME}.h")
    set(_outputs "${_stamp}")
    set(_byproducts "")
    if(GHS_LAYOUT STREQUAL "Full" OR GHS_LAYOUT STREQUAL "PrivateOnly")
        foreach(_rel IN LISTS _header_rel)
            list(APPEND _byproducts "${_private_include}/${_rel}")
        endforeach()
        list(APPEND _byproducts "${_private_include}/${GHS_MODULE}_libdefs.h")
    endif()
    if(GHS_LAYOUT STREQUAL "Full" OR GHS_LAYOUT STREQUAL "PublicOnly")
        foreach(_rel IN LISTS _header_rel)
            list(APPEND _byproducts "${_geninc_root}/${_rel}")
            list(APPEND _byproducts "${_sdk_root}/${_rel}")
        endforeach()
    endif()
    list(APPEND _outputs ${_byproducts})
    list(REMOVE_DUPLICATES _outputs)

    foreach(_output IN LISTS _outputs)
        set(_private_include_path "${_private_include}")
        set(_geninc_root_path "${_geninc_root}")
        set(_sdk_root_path "${_sdk_root}")
        cmake_path(IS_PREFIX _private_include_path "${_output}" NORMALIZE _is_private)
        cmake_path(IS_PREFIX _geninc_root_path "${_output}" NORMALIZE _is_geninc)
        cmake_path(IS_PREFIX _sdk_root_path "${_output}" NORMALIZE _is_sdk)
        if(_output STREQUAL _stamp OR _is_private)
            set(_boundary "${_build_root}")
        elseif(_is_geninc)
            set(_boundary "${_geninc_root}")
        elseif(_is_sdk)
            set(_boundary "${_sdk_root}")
        else()
            message(FATAL_ERROR
                "aros_genmodule_header_stamp: internal output path '${_output}' is not rooted")
        endif()
        _aros_genmodule_header_check_output("${_output}" "${_boundary}" "${GHS_NAME}")
        string(SHA256 _claim_hash "${_output}")
        set(_claim_property "AROS_GENMODULE_HEADER_RULE_OUTPUT_${_claim_hash}")
        get_property(_claimed GLOBAL PROPERTY "${_claim_property}" SET)
        if(_claimed)
            get_property(_previous GLOBAL PROPERTY "${_claim_property}")
            message(FATAL_ERROR
                "aros_genmodule_header_stamp: output '${_output}' already claimed by ${_previous}")
        endif()
        set_property(GLOBAL PROPERTY "${_claim_property}" "${GHS_NAME}")
    endforeach()

    set(_mkdir_dirs "${_private_module_root}" "${_geninc_root}" "${_sdk_root}")
    if(GHS_LAYOUT STREQUAL "Full" OR GHS_LAYOUT STREQUAL "PrivateOnly")
        list(APPEND _mkdir_dirs "${_private_include}")
        foreach(_rel IN LISTS _header_rel)
            get_filename_component(_subdir "${_rel}" DIRECTORY)
            list(APPEND _mkdir_dirs "${_private_include}/${_subdir}")
        endforeach()
    endif()
    if(GHS_LAYOUT STREQUAL "Full" OR GHS_LAYOUT STREQUAL "PublicOnly")
        foreach(_rel IN LISTS _header_rel)
            get_filename_component(_subdir "${_rel}" DIRECTORY)
            list(APPEND _mkdir_dirs "${_geninc_root}/${_subdir}"
                "${_sdk_root}/${_subdir}")
        endforeach()
    endif()
    list(REMOVE_DUPLICATES _mkdir_dirs)

    set(_genmodule_commands)
    if(GHS_LAYOUT STREQUAL "Full" OR GHS_LAYOUT STREQUAL "PrivateOnly")
        list(APPEND _genmodule_commands
            COMMAND "${AROS_HOST_GENMODULE}" -c "${_config_real}"
                -d "${_private_include}" writeincludes "${GHS_MODULE}" "${GHS_MODTYPE}"
            COMMAND "${AROS_HOST_GENMODULE}" -c "${_config_real}"
                -d "${_private_include}" writelibdefs "${GHS_MODULE}" "${GHS_MODTYPE}")
    endif()
    if(GHS_LAYOUT STREQUAL "Full" OR GHS_LAYOUT STREQUAL "PublicOnly")
        list(APPEND _genmodule_commands
            COMMAND "${AROS_HOST_GENMODULE}" -c "${_config_real}"
                -d "${_geninc_root}" writeincludes "${GHS_MODULE}" "${GHS_MODTYPE}"
            COMMAND "${AROS_HOST_GENMODULE}" -c "${_config_real}"
                -d "${_sdk_root}" writeincludes "${GHS_MODULE}" "${GHS_MODTYPE}")
    endif()

    add_custom_command(
        OUTPUT "${_stamp}"
        BYPRODUCTS ${_byproducts}
        COMMAND "${CMAKE_COMMAND}" -E make_directory ${_mkdir_dirs}
        ${_genmodule_commands}
        COMMAND "${CMAKE_COMMAND}" -E touch "${_stamp}"
        DEPENDS "${AROS_HOST_GENMODULE}" "${_config_real}"
        COMMENT "Generating public headers for ${GHS_MODULE} (${GHS_NAME})"
        VERBATIM)
    add_custom_target("${GHS_NAME}" DEPENDS "${_stamp}")
endfunction()
