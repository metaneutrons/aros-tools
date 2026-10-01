cmake_minimum_required(VERSION 3.22)
project(ExternalCMakeFixture C)
# This fixture stubs BootstrapSDK; its directory has to come first.
list(INSERT CMAKE_MODULE_PATH 0 "${CMAKE_CURRENT_LIST_DIR}/cmake")

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_PORTS_SOURCE_DIR "${CMAKE_CURRENT_LIST_DIR}/archive"
    CACHE PATH "fixture archive root")
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")

set(_archive "${CMAKE_CURRENT_LIST_DIR}/archive/source.txt")
if(EXTERNAL_CMAKE_CASE MATCHES "^component-")
    set(_upstream "${CMAKE_CURRENT_LIST_DIR}/components/upstream")
else()
    set(_upstream "${CMAKE_CURRENT_LIST_DIR}/upstream")
endif()

add_custom_target(fixture-fetch)
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_DESTINATION "${_upstream}")
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_COMPLETION_STAMP "${_archive}")

if(EXTERNAL_CMAKE_CASE STREQUAL "missing-fetch")
    set(_fetch_target absent-fetch)
else()
    set(_fetch_target fixture-fetch)
endif()
if(EXTERNAL_CMAKE_CASE STREQUAL "escape-binary")
    set(_binary "${CMAKE_BINARY_DIR}/../escape")
elseif(EXTERNAL_CMAKE_CASE MATCHES "^component-")
    set(_binary
        "${CMAKE_BINARY_DIR}/gen/external-cmake/fixture-components")
else()
    set(_binary
        "${CMAKE_BINARY_DIR}/gen/external-cmake/fixture-external")
endif()
if(EXTERNAL_CMAKE_CASE STREQUAL "overlap-prefix")
    set(_prefix "${_binary}/install")
else()
    set(_prefix "${CMAKE_BINARY_DIR}/install")
endif()
set(_library "${_prefix}/lib/libtiny.a")
set(_header "${_prefix}/include/tiny.h")
set(_metadata "${_prefix}/share/tiny/metadata.txt")
if(EXTERNAL_CMAKE_CASE STREQUAL "escape-product")
    set(_library "${CMAKE_BINARY_DIR}/outside/libtiny.a")
elseif(EXTERNAL_CMAKE_CASE STREQUAL "missing-output")
    set(_header "${_prefix}/include/not-installed.h")
endif()
if(EXTERNAL_CMAKE_CASE STREQUAL "unsafe-option")
    set(_options "-Pbad.cmake")
elseif(EXTERNAL_CMAKE_CASE STREQUAL "toolchain-override")
    set(_options "-DCMAKE_C_COMPILER=/tmp/not-allowed")
else()
    set(_options "-DMINI_FEATURE=yes")
endif()

if(EXTERNAL_CMAKE_CASE MATCHES "^component-")
    set(_left_library "${_prefix}/lib/libleft.a")
    set(_right_library "${_prefix}/lib/libright.a")
    if(EXTERNAL_CMAKE_CASE STREQUAL "component-missing-product")
        set(_right_library "${_prefix}/lib/libunbuilt.a")
    endif()
    set(_component_arguments
        LIBRARY_GROUP
        BUILD_TARGETS left right
        INSTALL_COMPONENTS left-headers right-headers
        COMPILE_DEFINES EXTERNAL_COMPONENT_FIXTURE=1)
    set(_host_tools "")
    if(EXTERNAL_CMAKE_CASE STREQUAL "component-invalid-target")
        set(_component_arguments
            LIBRARY_GROUP
            BUILD_TARGETS left/bad right
            INSTALL_COMPONENTS left-headers right-headers
            COMPILE_DEFINES EXTERNAL_COMPONENT_FIXTURE=1)
    elseif(EXTERNAL_CMAKE_CASE STREQUAL "component-invalid-install")
        set(_component_arguments
            LIBRARY_GROUP
            BUILD_TARGETS left right
            INSTALL_COMPONENTS left-headers bad/name
            COMPILE_DEFINES EXTERNAL_COMPONENT_FIXTURE=1)
    elseif(EXTERNAL_CMAKE_CASE STREQUAL "component-group-no-targets")
        set(_component_arguments
            LIBRARY_GROUP
            INSTALL_COMPONENTS left-headers right-headers)
    elseif(EXTERNAL_CMAKE_CASE STREQUAL "component-group-no-components")
        set(_component_arguments
            LIBRARY_GROUP
            BUILD_TARGETS left right)
    elseif(EXTERNAL_CMAKE_CASE STREQUAL "component-host-tool-missing")
        set(AROS_CROSS_TOOLCHAIN_ROOT "${CMAKE_BINARY_DIR}/missing-toolchain")
        set(_host_tools HOST_TOOLS FIXTURE_TOOL=missing-tool@11.0.0)
    elseif(EXTERNAL_CMAKE_CASE STREQUAL "component-host-tool-wrong-version")
        set(AROS_CROSS_TOOLCHAIN_ROOT "${CMAKE_BINARY_DIR}/wrong-version-toolchain")
        file(MAKE_DIRECTORY "${AROS_CROSS_TOOLCHAIN_ROOT}/bin")
        file(CREATE_LINK "${EXTERNAL_TEST_CLANG}"
            "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/llvm-ar" SYMBOLIC)
        set(_host_tools HOST_TOOLS FIXTURE_TOOL=llvm-ar@11.0.0)
    elseif(EXTERNAL_CMAKE_CASE STREQUAL "component-host-tool-version-prefix")
        set(AROS_CROSS_TOOLCHAIN_ROOT "${CMAKE_BINARY_DIR}/prefix-version-toolchain")
        file(MAKE_DIRECTORY "${AROS_CROSS_TOOLCHAIN_ROOT}/bin")
        file(CREATE_LINK
            "${CMAKE_CURRENT_LIST_DIR}/host-tools/llvm-ar-version-11.0.01.sh"
            "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/llvm-ar" SYMBOLIC)
        set(_host_tools HOST_TOOLS FIXTURE_TOOL=llvm-ar@11.0.0)
    elseif(EXTERNAL_CMAKE_CASE STREQUAL "component-host-tool-duplicate-key")
        set(AROS_CROSS_TOOLCHAIN_ROOT "${CMAKE_BINARY_DIR}/duplicate-key-toolchain")
        file(MAKE_DIRECTORY "${AROS_CROSS_TOOLCHAIN_ROOT}/bin")
        file(CREATE_LINK
            "${CMAKE_CURRENT_LIST_DIR}/host-tools/llvm-ar-version-11.0.0.sh"
            "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/llvm-ar" SYMBOLIC)
        set(_host_tools HOST_TOOLS
            FIXTURE_TOOL=llvm-ar@11.0.0 FIXTURE_TOOL=missing-tool@11.0.0)
    endif()

    if(EXTERNAL_CMAKE_CASE STREQUAL "component-success")
        set(_generated_public_header
            "${CMAKE_BINARY_DIR}/generated/fixture_public.h")
        add_custom_command(
            OUTPUT "${_generated_public_header}"
            COMMAND "${CMAKE_COMMAND}" -E make_directory
                "${CMAKE_BINARY_DIR}/generated"
            COMMAND "${CMAKE_COMMAND}" -E touch
                "${_generated_public_header}"
            COMMENT "Generating fixture public SDK header"
            VERBATIM)
        add_custom_target(fixture-public-headers
            DEPENDS "${_generated_public_header}")
        _aros_register_genmodule_public_includes(fixture-public-headers)
    endif()

    aros_build_external_cmake(
        MMAKE_ID fixture-components
        SOURCE_DIR "${_upstream}"
        BINARY_DIR "${_binary}"
        INSTALL_PREFIX "${_prefix}"
        FETCH_TARGET "${_fetch_target}"
        PROVIDED_LIBRARY components
        OPTIONS "${_options}"
        LIBRARY_PRODUCTS "${_left_library}" "${_right_library}"
        HEADER_PRODUCTS "${_prefix}/include/left.h" "${_prefix}/include/right.h"
        PUBLIC_INCLUDE_DIRS "${_prefix}/include"
        ${_component_arguments}
        ${_host_tools})

    if(EXTERNAL_CMAKE_CASE STREQUAL "component-success")
        function(_check_fixture_external_component_barrier)
            get_target_property(_producer_dependencies fixture-components
                MANUALLY_ADDED_DEPENDENCIES)
            if(NOT "aros-genmodule-public-includes" IN_LIST
               _producer_dependencies)
                message(FATAL_ERROR
                    "external component producer lacks the public SDK header barrier")
            endif()
        endfunction()
        cmake_language(DEFER CALL _check_fixture_external_component_barrier)

        add_library(external-consumer STATIC components/consumer.c)
        target_link_libraries(external-consumer PRIVATE
            fixture-components-external-components)
        get_target_property(_consumer_libraries external-consumer LINK_LIBRARIES)
        if(NOT _consumer_libraries STREQUAL
           "fixture-components-external-components")
            message(FATAL_ERROR
                "component consumer must link the component interface as one target")
        endif()
        get_target_property(_component_libraries
            fixture-components-external-components INTERFACE_LINK_LIBRARIES)
        if(AROS_COLLECT_BIN)
            set(_group_start "--start-group")
            set(_group_end "--end-group")
        else()
            set(_group_start "-Wl,--start-group")
            set(_group_end "-Wl,--end-group")
        endif()
        set(_expected_component_libraries
            "${_group_start};${_left_library};${_right_library};${_group_end}")
        if(NOT "${_component_libraries}" STREQUAL
           "${_expected_component_libraries}")
            message(FATAL_ERROR
                "component archives are not linked as one ordered group: ${_component_libraries}")
        endif()
        # The AROS collector invokes ld.lld directly, so remove the host
        # platform's default CMake driver flags from this cross-link probe.
        set(CMAKE_C_LINK_FLAGS "")
        add_executable(external-component-link-consumer EXCLUDE_FROM_ALL
            components/link-consumer.c)
        target_link_libraries(external-component-link-consumer PRIVATE
            fixture-components-external-components)
    endif()
else()
    aros_build_external_cmake(
        MMAKE_ID fixture-external
        SOURCE_DIR "${_upstream}"
        BINARY_DIR "${_binary}"
        INSTALL_PREFIX "${_prefix}"
        FETCH_TARGET "${_fetch_target}"
        PROVIDED_LIBRARY tiny
        OPTIONS "${_options}"
        LIBRARY_PRODUCTS "${_library}"
        HEADER_PRODUCTS "${_header}"
        AUXILIARY_PRODUCTS "${_metadata}"
        PUBLIC_INCLUDE_DIRS "${_prefix}/include")
endif()

if(EXTERNAL_CMAKE_CASE STREQUAL "collision" OR
   EXTERNAL_CMAKE_CASE STREQUAL "binary-collision")
    if(EXTERNAL_CMAKE_CASE STREQUAL "binary-collision")
        set(_second_binary "${_binary}")
    else()
        set(_second_binary
            "${CMAKE_BINARY_DIR}/gen/external-cmake/fixture-external-second")
    endif()
    aros_build_external_cmake(
        MMAKE_ID fixture-external-second
        SOURCE_DIR "${_upstream}"
        BINARY_DIR "${_second_binary}"
        INSTALL_PREFIX "${_prefix}"
        FETCH_TARGET fixture-fetch
        PROVIDED_LIBRARY tiny
        OPTIONS -DMINI_FEATURE=yes
        LIBRARY_PRODUCTS "${_prefix}/lib/libtiny.a"
        HEADER_PRODUCTS "${_prefix}/include/tiny.h"
        AUXILIARY_PRODUCTS "${_prefix}/share/tiny/metadata.txt"
        PUBLIC_INCLUDE_DIRS "${_prefix}/include")
endif()

if(NOT EXTERNAL_CMAKE_CASE MATCHES "^component-")
    add_library(external-consumer STATIC consumer.c)
    target_link_libraries(external-consumer PRIVATE
        fixture-external-external-tiny)
endif()
