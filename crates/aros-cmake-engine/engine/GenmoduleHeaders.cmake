# Exact, closure-owned genmodule public headers.
#
# BootstrapSDK's Rust scan is intentionally broad and cannot decide which
# same-named module owns a shared SDK header in a particular target closure.
# This helper mirrors the reference `<mmake>-includes` rule instead: it runs
# the legacy host generator against one declared .conf into a private tree,
# then publishes the resulting public headers only when that concrete includes
# target is requested.

include_guard(GLOBAL)
include(CMakeParseArguments)

# aros_materialize_genmodule_headers(
#     NAME <generated-target> OWNER <existing-includes-target>
#     CONFIG <absolute-or-source-relative-.conf> MODULE <module-name>
#     MODTYPE <genmodule-type> INCLUDE_NAME <published-header-basename>)
function(aros_materialize_genmodule_headers)
    set(oneValueArgs NAME OWNER CONFIG MODULE MODTYPE INCLUDE_NAME)
    cmake_parse_arguments(GMH "" "${oneValueArgs}" "" ${ARGN})

    if(GMH_UNPARSED_ARGUMENTS OR GMH_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_materialize_genmodule_headers: malformed arguments: "
            "${GMH_UNPARSED_ARGUMENTS}${GMH_KEYWORDS_MISSING_VALUES}")
    endif()
    foreach(_required NAME OWNER CONFIG MODULE MODTYPE INCLUDE_NAME)
        if(NOT GMH_${_required})
            message(FATAL_ERROR
                "aros_materialize_genmodule_headers: ${_required} is required")
        endif()
    endforeach()
    if(NOT TARGET "${GMH_OWNER}")
        message(FATAL_ERROR
            "aros_materialize_genmodule_headers: owner target ${GMH_OWNER} does not exist")
    endif()
    if(TARGET "${GMH_NAME}")
        message(FATAL_ERROR
            "aros_materialize_genmodule_headers: generated target ${GMH_NAME} already exists")
    endif()
    if(NOT AROS_HOST_GENMODULE)
        message(FATAL_ERROR
            "aros_materialize_genmodule_headers: legacy host genmodule was not registered")
    endif()
    if(NOT IS_ABSOLUTE "${AROS_HOST_GENMODULE}")
        message(FATAL_ERROR
            "aros_materialize_genmodule_headers: AROS_HOST_GENMODULE must be absolute")
    endif()

    if(IS_ABSOLUTE "${GMH_CONFIG}")
        set(_config "${GMH_CONFIG}")
    else()
        set(_config "${CMAKE_SOURCE_DIR}/${GMH_CONFIG}")
    endif()
    cmake_path(NORMAL_PATH _config)
    if(NOT EXISTS "${_config}" OR IS_DIRECTORY "${_config}" OR
       IS_SYMLINK "${_config}")
        message(FATAL_ERROR
            "aros_materialize_genmodule_headers: CONFIG is missing or not a regular file: ${_config}")
    endif()

    foreach(_field MODULE MODTYPE INCLUDE_NAME)
        if(GMH_${_field} MATCHES "[/\\\\;]" OR
           GMH_${_field} STREQUAL "." OR GMH_${_field} STREQUAL "..")
            message(FATAL_ERROR
                "aros_materialize_genmodule_headers: ${_field} is not a header-safe basename: ${GMH_${_field}}")
        endif()
    endforeach()
    foreach(_root_var AROS_SDK_INCLUDE_DIR AROS_GENINC_DIR
            AROS_DEVELOPER_INCLUDE_DIR)
        if(NOT DEFINED ${_root_var} OR "${${_root_var}}" STREQUAL "")
            message(FATAL_ERROR
                "aros_materialize_genmodule_headers: ${_root_var} is required")
        endif()
        if(NOT IS_ABSOLUTE "${${_root_var}}")
            message(FATAL_ERROR
                "aros_materialize_genmodule_headers: ${_root_var} must be absolute")
        endif()
    endforeach()

    string(MAKE_C_IDENTIFIER "${GMH_NAME}" _safe_name)
    set(_private_root "${CMAKE_BINARY_DIR}/genmodule-headers/${_safe_name}")
    set(_private_include "${_private_root}/include")
    set(_header_rel
        "clib/${GMH_INCLUDE_NAME}_protos.h"
        "inline/${GMH_INCLUDE_NAME}.h"
        "defines/${GMH_INCLUDE_NAME}.h"
        "defines/${GMH_INCLUDE_NAME}_LVO.h"
        "proto/${GMH_INCLUDE_NAME}.h")

    set(_private_headers "")
    set(_private_dirs "")
    set(_published_headers "")
    set(_publish_commands "")
    foreach(_rel IN LISTS _header_rel)
        set(_private "${_private_include}/${_rel}")
        list(APPEND _private_headers "${_private}")
        get_filename_component(_private_dir "${_private}" DIRECTORY)
        list(APPEND _private_dirs "${_private_dir}")
        foreach(_public_root
                "${AROS_SDK_INCLUDE_DIR}"
                "${AROS_GENINC_DIR}"
                "${AROS_DEVELOPER_INCLUDE_DIR}")
            set(_public "${_public_root}/${_rel}")
            list(APPEND _published_headers "${_public}")
            get_filename_component(_public_dir "${_public}" DIRECTORY)
            list(APPEND _publish_commands
                COMMAND "${CMAKE_COMMAND}" -E make_directory "${_public_dir}"
                COMMAND "${CMAKE_COMMAND}" -E copy_if_different
                    "${_private}" "${_public}")
        endforeach()
    endforeach()
    list(REMOVE_DUPLICATES _private_dirs)

    # A shared public pathname may only have one CMake producer.  A second
    # declaration is not resolved by alphabetical or module-type precedence:
    # the legacy closure must name the appropriate owner explicitly.
    string(JOIN "|" _signature
        "${_config}" "${GMH_MODULE}" "${GMH_MODTYPE}" "${GMH_INCLUDE_NAME}")
    foreach(_public IN LISTS _published_headers)
        string(SHA256 _public_hash "${_public}")
        set(_claim_property "AROS_GENMODULE_HEADER_CLAIM_${_public_hash}")
        get_property(_claimed GLOBAL PROPERTY "${_claim_property}" SET)
        if(_claimed)
            get_property(_previous GLOBAL PROPERTY "${_claim_property}")
            if(NOT "${_previous}" STREQUAL "${_signature}")
                message(FATAL_ERROR
                    "aros_materialize_genmodule_headers: ${_public} has conflicting producers")
            endif()
            message(FATAL_ERROR
                "aros_materialize_genmodule_headers: ${_public} was registered twice")
        endif()
        set_property(GLOBAL PROPERTY "${_claim_property}" "${_signature}")
    endforeach()

    # The bootstrap scan may have produced an ambiguous stale copy. Removing
    # precisely these declared outputs makes Ninja require this closure owner.
    file(REMOVE ${_published_headers})
    add_custom_command(
        OUTPUT ${_private_headers} ${_published_headers}
        COMMAND "${CMAKE_COMMAND}" -E make_directory
            "${_private_include}" ${_private_dirs}
        COMMAND "${AROS_HOST_GENMODULE}" -c "${_config}" -d "${_private_include}"
            writeincludes "${GMH_MODULE}" "${GMH_MODTYPE}"
        ${_publish_commands}
        DEPENDS "${AROS_HOST_GENMODULE}" "${_config}"
        COMMENT "Generating exact ${GMH_MODULE}.${GMH_MODTYPE} public headers"
        VERBATIM)

    add_custom_target("${GMH_NAME}" DEPENDS ${_published_headers})
    add_dependencies("${GMH_OWNER}" "${GMH_NAME}")
endfunction()
