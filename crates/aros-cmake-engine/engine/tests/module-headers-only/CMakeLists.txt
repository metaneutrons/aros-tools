cmake_minimum_required(VERSION 3.22)
project(ModuleHeadersOnlyFixture C)

if(NOT ENGINE_DIR OR NOT TEST_CASE)
    message(FATAL_ERROR "ENGINE_DIR and TEST_CASE are required")
endif()

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/source")

# The fixture exercises only the module builder, not SDK bootstrap. The engine
# loads this function through the regular module path just as other focused
# AROS.cmake fixtures do.
set(_bootstrap_stub "${CMAKE_CURRENT_BINARY_DIR}/bootstrap-stub")
file(MAKE_DIRECTORY "${_bootstrap_stub}")
file(WRITE "${_bootstrap_stub}/BootstrapSDK.cmake"
    "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
list(INSERT CMAKE_MODULE_PATH 0 "${_bootstrap_stub}")
include("${ENGINE_DIR}/AROS.cmake")

# Replace only the expensive reference generator wiring with two real bounded
# custom-command outputs. Keep the config/type/ABI checks that the production
# helper owns, then let the production headers-only builder wire those outputs.
function(_aros_generate_module_support out_prefix)
    set(options ABI SOURCES_ONLY)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY MODTYPE MODSUFFIX)
    cmake_parse_arguments(PARSE_ARGV 1 GM
        "${options}" "${oneValueArgs}" "")
    if(GM_UNPARSED_ARGUMENTS OR GM_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "fixture generator received malformed arguments")
    endif()
    if(NOT GM_ABI OR GM_SOURCES_ONLY)
        message(FATAL_ERROR "headers-only builder did not request ABI generation")
    endif()
    if(NOT GM_TARGET OR NOT GM_MMAKE_ID OR NOT GM_DIRECTORY OR NOT GM_MODTYPE)
        message(FATAL_ERROR "fixture generator requires module declaration fields")
    endif()
    if(NOT GM_MODTYPE IN_LIST AROS_GENMODULE_MODTYPES)
        message(FATAL_ERROR "fixture generator received unsupported module type")
    endif()

    cmake_path(ABSOLUTE_PATH GM_DIRECTORY NORMALIZE OUTPUT_VARIABLE _directory)
    _aros_module_config(_config "${GM_MMAKE_ID}" "${_directory}" "${GM_TARGET}")
    if(NOT EXISTS "${_config}")
        message(FATAL_ERROR "${GM_MMAKE_ID}: missing genmodule config ${_config}")
    endif()

    set(_output_dir "${CMAKE_CURRENT_BINARY_DIR}/fake-genmodule/${GM_MMAKE_ID}")
    set(_includes_output "${_output_dir}/includes.stamp")
    set(_fd_output "${_output_dir}/fd.stamp")
    add_custom_command(
        OUTPUT "${_includes_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
        COMMAND "${CMAKE_COMMAND}" -E touch "${_includes_output}"
        DEPENDS "${_config}"
        VERBATIM)
    set(_includes_target "${GM_MMAKE_ID}-includes-generated")
    add_custom_target("${_includes_target}" DEPENDS "${_includes_output}")
    set(_fd_target "")
    if(NOT GM_TARGET STREQUAL "ProbeNoFd")
        add_custom_command(
            OUTPUT "${_fd_output}"
            COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
            COMMAND "${CMAKE_COMMAND}" -E touch "${_fd_output}"
            DEPENDS "${_config}"
            VERBATIM)
        add_custom_target("${GM_MMAKE_ID}-fd-generated" DEPENDS "${_fd_output}")
        set(_fd_target "${GM_MMAKE_ID}-fd-generated")
    endif()

    set(${out_prefix}_INCLUDES_TARGET "${_includes_target}" PARENT_SCOPE)
    set(${out_prefix}_FD_TARGET "${_fd_target}" PARENT_SCOPE)
    set(${out_prefix}_HEADERS "${_includes_output}" PARENT_SCOPE)
endfunction()

set(_module_directory "${AROS_SOURCE_DIR}/modules")
if(TEST_CASE STREQUAL "valid")
    aros_add_module_headers(
        TARGET Probe
        MMAKE_ID probe-mmake
        DIRECTORY "${_module_directory}"
        MODTYPE library
        MODSUFFIX library)

    foreach(_target probe-mmake-includes probe-mmake-fd
            includes-Probe includes-Probe_rel includes-all
            probe-mmake-includes-generated probe-mmake-fd-generated
            aros-genmodule-public-includes)
        if(NOT TARGET "${_target}")
            message(FATAL_ERROR "headers-only builder did not declare ${_target}")
        endif()
    endforeach()
    foreach(_forbidden probe-mmake probe-mmake-linklib linklibs-Probe
            linklibs-Probe_rel)
        if(TARGET "${_forbidden}")
            message(FATAL_ERROR "headers-only builder declared forbidden target ${_forbidden}")
        endif()
    endforeach()

    foreach(_alias probe-mmake-includes probe-mmake-fd includes-Probe
            includes-Probe_rel includes-all)
        get_property(_dependencies TARGET "${_alias}"
            PROPERTY MANUALLY_ADDED_DEPENDENCIES)
        if(_alias STREQUAL "probe-mmake-includes")
            foreach(_required probe-mmake-includes-generated probe-mmake-fd-generated)
                if(NOT _required IN_LIST _dependencies)
                    message(FATAL_ERROR
                        "${_alias} does not depend on real ABI producer ${_required}: "
                        "${_dependencies}")
                endif()
            endforeach()
        elseif(_alias STREQUAL "probe-mmake-fd")
            if(NOT probe-mmake-fd-generated IN_LIST _dependencies)
                message(FATAL_ERROR "FD alias lost its real generator dependency")
            endif()
        elseif(NOT probe-mmake-includes IN_LIST _dependencies)
            message(FATAL_ERROR
                "${_alias} does not resolve through probe-mmake-includes: ${_dependencies}")
        endif()
    endforeach()

    get_property(_public_dependencies TARGET aros-genmodule-public-includes
        PROPERTY MANUALLY_ADDED_DEPENDENCIES)
    if(NOT probe-mmake-includes-generated IN_LIST _public_dependencies)
        message(FATAL_ERROR "public includes barrier lacks the generated headers owner")
    endif()

    # A real includes-only config may have no exported functions. Preserve its
    # concrete includes owner, but do not fabricate an FD target or alias.
    aros_add_module_headers(
        TARGET ProbeNoFd
        MMAKE_ID probe-nofd
        DIRECTORY "${_module_directory}"
        MODTYPE library)
    foreach(_target probe-nofd-includes includes-ProbeNoFd
            includes-ProbeNoFd_rel probe-nofd-includes-generated)
        if(NOT TARGET "${_target}")
            message(FATAL_ERROR "no-FD headers-only builder did not declare ${_target}")
        endif()
    endforeach()
    foreach(_forbidden probe-nofd-fd probe-nofd-fd-generated
            probe-nofd-linklib linklibs-ProbeNoFd linklibs-ProbeNoFd_rel)
        if(TARGET "${_forbidden}")
            message(FATAL_ERROR
                "no-FD headers-only builder declared nonexistent ${_forbidden}")
        endif()
    endforeach()
elseif(TEST_CASE STREQUAL "missing-config")
    aros_add_module_headers(
        TARGET Missing
        MMAKE_ID missing-mmake
        DIRECTORY "${AROS_SOURCE_DIR}/missing"
        MODTYPE library)
elseif(TEST_CASE STREQUAL "missing-required")
    aros_add_module_headers(
        TARGET Probe
        MMAKE_ID probe-mmake
        DIRECTORY "${_module_directory}")
elseif(TEST_CASE STREQUAL "runtime-input")
    aros_add_module_headers(
        TARGET Probe
        MMAKE_ID probe-mmake
        DIRECTORY "${_module_directory}"
        MODTYPE library
        SOURCES module.c)
else()
    message(FATAL_ERROR "unknown test case ${TEST_CASE}")
endif()
