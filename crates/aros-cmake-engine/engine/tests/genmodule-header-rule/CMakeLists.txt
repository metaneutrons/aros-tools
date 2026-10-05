cmake_minimum_required(VERSION 3.24)
project(GenmoduleHeaderRuleFixture NONE)

foreach(_required AROS_SOURCE_DIR AROS_ENGINE_DIR AROS_HOST_GENMODULE)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "${_required} is required")
    endif()
endforeach()

include("${AROS_ENGINE_DIR}/GenmoduleHeaderRules.cmake")

set(AROS_BUILD_DIR "${CMAKE_BINARY_DIR}")
if(DEFINED AROS_TEST_GENINC_DIR AND NOT "${AROS_TEST_GENINC_DIR}" STREQUAL "")
    set(AROS_GENINC_DIR "${AROS_TEST_GENINC_DIR}")
else()
    set(AROS_GENINC_DIR "${CMAKE_BINARY_DIR}/GENINCDIR")
endif()
set(AROS_SDK_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SDK/include")

if(NOT DEFINED AROS_HEADER_PROBE)
    set(AROS_HEADER_PROBE "positive")
endif()
if(NOT DEFINED AROS_HEADER_CONFIG)
    set(AROS_HEADER_CONFIG "rom/kernel/clocksource.conf")
endif()

# Leave the symlink probe's GENINC root uncreated: the helper must inspect the
# path from CMAKE_BINARY_DIR and reject its redirect ancestor before touching it.
if(NOT AROS_HEADER_PROBE STREQUAL "symlink")
    file(MAKE_DIRECTORY "${AROS_GENINC_DIR}")
endif()
file(MAKE_DIRECTORY "${AROS_SDK_INCLUDE_DIR}")

function(_add_clocksource_rule name config)
    aros_genmodule_header_stamp(
        NAME "${name}"
        LAYOUT Full
        DECLARING_DIR rom/kernel
        CONFIG "${config}"
        MODULE clocksource
        MODTYPE resource
        INCLUDE_NAME clocksource)
endfunction()

if(AROS_HEADER_PROBE STREQUAL "positive")
    aros_genmodule_header_stamp(
        NAME kernel-clocksource-gen-includes
        LAYOUT Full
        DECLARING_DIR rom/kernel
        CONFIG rom/kernel/clocksource.conf
        MODULE clocksource
        MODTYPE resource
        INCLUDE_NAME clocksource)
    aros_genmodule_header_stamp(
        NAME workbench-libs-gl-gen-includes
        LAYOUT Full
        DECLARING_DIR workbench/libs/gl
        CONFIG workbench/libs/gl/gl.conf
        MODULE gl
        MODTYPE library
        INCLUDE_NAME gl)
    aros_genmodule_header_stamp(
        NAME workbench-tools-sysexplorer-gen-includes
        LAYOUT PrivateOnly
        DECLARING_DIR workbench/tools/SysExplorer
        CONFIG workbench/tools/SysExplorer/sysexp.conf
        MODULE sysexp
        MODTYPE library
        INCLUDE_NAME sysexp)
    aros_genmodule_header_stamp(
        NAME workbench-prefs-network-gen-includes
        LAYOUT PrivateOnly
        DECLARING_DIR workbench/prefs/network
        CONFIG workbench/prefs/network/netprefs.conf
        MODULE netprefs
        MODTYPE library
        INCLUDE_NAME netprefs)
    aros_genmodule_header_stamp(
        NAME workbench-libs-vhi-includes
        LAYOUT PublicOnly
        DECLARING_DIR workbench/libs/vhi
        CONFIG workbench/libs/vhi/vhi.conf
        MODULE vhi
        MODTYPE library
        INCLUDE_NAME vhi)

    add_custom_target(all-header-rules)
    add_dependencies(all-header-rules
        kernel-clocksource-gen-includes
        workbench-libs-gl-gen-includes
        workbench-tools-sysexplorer-gen-includes
        workbench-prefs-network-gen-includes
        workbench-libs-vhi-includes)
else()
    if(AROS_HEADER_PROBE STREQUAL "path")
        set(_config "rom/kernel/../clocksource.conf")
    else()
        set(_config "${AROS_HEADER_CONFIG}")
    endif()
    _add_clocksource_rule(kernel-clocksource-gen-includes "${_config}")
    if(AROS_HEADER_PROBE STREQUAL "duplicate")
        _add_clocksource_rule(duplicate-clocksource-gen-includes "${_config}")
    endif()
endif()
