# Named source-owned writefiles endpoints. The actual output grammar and writer
# are shared with generated client archives; no target or board name is special.
include_guard(GLOBAL)
include(CMakeParseArguments)
include("${CMAKE_CURRENT_LIST_DIR}/GenmoduleHeaderRules.cmake")

function(aros_genmodule_writefiles_stamp)
    cmake_parse_arguments(GWS "" "NAME;CONFIG;MODULE;MODTYPE" "" ${ARGN})
    if(GWS_UNPARSED_ARGUMENTS OR GWS_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "aros_genmodule_writefiles_stamp: malformed arguments")
    endif()
    foreach(_key NAME CONFIG MODULE MODTYPE)
        if(NOT DEFINED GWS_${_key} OR "${GWS_${_key}}" STREQUAL "")
            message(FATAL_ERROR "aros_genmodule_writefiles_stamp: ${_key} is required")
        endif()
    endforeach()
    if(NOT "${GWS_NAME}" MATCHES "^[A-Za-z0-9_][A-Za-z0-9_.+-]*$" OR
       TARGET "${GWS_NAME}")
        message(FATAL_ERROR "aros_genmodule_writefiles_stamp: unsafe or duplicate target")
    endif()
    if(NOT "${GWS_MODULE}" MATCHES "^[A-Za-z0-9_-]+$" OR
       NOT "${GWS_MODTYPE}" MATCHES "^[A-Za-z0-9_-]+$")
        message(FATAL_ERROR "aros_genmodule_writefiles_stamp: unsafe module identity")
    endif()
    # The engine builds the host tool in the build step, so it need not exist
    # while this configures; the shared writer's command depends on it. Like
    # aros_genmodule_header_stamp, refuse a relative, directory or symlinked
    # path.
    if(NOT IS_ABSOLUTE "${AROS_SOURCE_DIR}" OR
       NOT IS_ABSOLUTE "${AROS_HOST_GENMODULE}" OR
       IS_DIRECTORY "${AROS_HOST_GENMODULE}" OR IS_SYMLINK "${AROS_HOST_GENMODULE}")
        message(FATAL_ERROR "aros_genmodule_writefiles_stamp: source and regular host tool are required")
    endif()
    _aros_genmodule_header_validate_relative("${GWS_CONFIG}" "CONFIG" FALSE)
    if(NOT "${GWS_CONFIG}" MATCHES "[.]conf$")
        message(FATAL_ERROR "aros_genmodule_writefiles_stamp: CONFIG must name a .conf file")
    endif()
    set(_component_path "${AROS_SOURCE_DIR}")
    string(REPLACE "/" ";" _config_components "${GWS_CONFIG}")
    foreach(_component IN LISTS _config_components)
        string(APPEND _component_path "/${_component}")
        if(IS_SYMLINK "${_component_path}")
            message(FATAL_ERROR "aros_genmodule_writefiles_stamp: config path crosses symlink")
        endif()
    endforeach()
    set(_config "${AROS_SOURCE_DIR}/${GWS_CONFIG}")
    if(NOT EXISTS "${_config}" OR IS_DIRECTORY "${_config}" OR IS_SYMLINK "${_config}")
        message(FATAL_ERROR "aros_genmodule_writefiles_stamp: config is not a regular source file")
    endif()
    file(REAL_PATH "${AROS_SOURCE_DIR}" _source_real)
    file(REAL_PATH "${_config}" _config_real)
    cmake_path(IS_PREFIX _source_real "${_config_real}" NORMALIZE _inside)
    if(NOT _inside)
        message(FATAL_ERROR "aros_genmodule_writefiles_stamp: config escapes source tree")
    endif()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${_config}")
    _aros_genmodule_linklib_sources(_outputs _writer _includes _bound_config
        "${AROS_SOURCE_DIR}"
        "@AROS_GENMODULE|all|stackstubs,regcallstubs,autoinit,getlibbase|${GWS_MODULE}|${GWS_MODTYPE}|${_config}")
    foreach(_output IN LISTS _outputs)
        _aros_genmodule_header_check_output("${_output}" "${CMAKE_BINARY_DIR}" "${GWS_NAME}")
    endforeach()
    foreach(_header
            "clib/${GWS_MODULE}_protos.h" "inline/${GWS_MODULE}.h"
            "defines/${GWS_MODULE}.h" "defines/${GWS_MODULE}_LVO.h"
            "proto/${GWS_MODULE}.h")
        _aros_genmodule_header_check_output("${_includes}/${_header}" "${CMAKE_BINARY_DIR}" "${GWS_NAME}")
    endforeach()
    add_custom_target("${GWS_NAME}")
    add_dependencies("${GWS_NAME}" "${_writer}")
endfunction()
