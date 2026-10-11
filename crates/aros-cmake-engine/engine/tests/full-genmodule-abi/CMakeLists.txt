cmake_minimum_required(VERSION 3.22)
project(FullGenmoduleAbiContract C)

# The full engine normally loads BootstrapSDK to populate broad SDK headers.
# This fixture only exercises genmodule output ownership, so keep that one
# bootstrap side effect out while loading the production AROS.cmake functions.
set(_bootstrap_stub_dir "${CMAKE_CURRENT_BINARY_DIR}/bootstrap-stub")
file(MAKE_DIRECTORY "${_bootstrap_stub_dir}")
file(WRITE "${_bootstrap_stub_dir}/BootstrapSDK.cmake"
    "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
list(INSERT CMAKE_MODULE_PATH 0 "${_bootstrap_stub_dir}")

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
include("${AROS_TEST_ENGINE_DIR}/tests/NativeSourceSelectionFixture.cmake")
if(AROS_ARCHIVE_PROBE STREQUAL "positive-consumer-suppressed")
    # Exercise a consumer binding without writing any of its inputs into the
    # caller's AROS checkout. The test owns this small source copy.
    set(_consumer_source_root "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-source")
    file(MAKE_DIRECTORY
        "${_consumer_source_root}/workbench/libs"
        "${_consumer_source_root}/compiler/startup"
        "${_consumer_source_root}/compiler/libinit")
    file(COPY "${AROS_SOURCE_DIR}/workbench/libs/tiff"
        DESTINATION "${_consumer_source_root}/workbench/libs")
    file(COPY "${AROS_SOURCE_DIR}/compiler/startup/startup.c"
        DESTINATION "${_consumer_source_root}/compiler/startup")
    file(COPY "${AROS_SOURCE_DIR}/compiler/libinit/libentry.c"
        DESTINATION "${_consumer_source_root}/compiler/libinit")
    set(AROS_SOURCE_DIR "${_consumer_source_root}")
    aros_test_prepare_native_consumer_selection("${AROS_SOURCE_DIR}"
        "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-contract-test.json")
    include("${AROS_TEST_ENGINE_DIR}/NativeConsumerContract.cmake")
    aros_validate_native_consumer_contract()
endif()
include("${AROS_TEST_ENGINE_DIR}/AROS.cmake")

# Exercise the production module-support producer with overlapping SDK and
# Developer roots, while retaining GENINCDIR as a distinct third destination.
# The driver configures both exact and lexical-alias spellings in fresh Ninja
# trees, then builds the real reference genmodule outputs.
if(AROS_PUBLICATION_ROOT_PROBE)
    if(NOT AROS_PUBLICATION_ROOT_PROBE MATCHES "^(coincident|lexical-alias)$")
        message(FATAL_ERROR
            "unknown public-root probe: ${AROS_PUBLICATION_ROOT_PROBE}")
    endif()
    set(_probe_module_dir "${AROS_SOURCE_DIR}/workbench/libs/tiff")
    set(_probe_public_root "${CMAKE_BINARY_DIR}/publication-roots/public")
    if(AROS_PUBLICATION_ROOT_PROBE STREQUAL "coincident")
        set(AROS_SDK_INCLUDE_DIR "${_probe_public_root}")
        set(AROS_DEVELOPER_INCLUDE_DIR "${_probe_public_root}")
    else()
        set(AROS_SDK_INCLUDE_DIR
            "${CMAKE_BINARY_DIR}/publication-roots/lexical/../public")
        set(AROS_DEVELOPER_INCLUDE_DIR "${_probe_public_root}")
    endif()
    set(AROS_GENINC_DIR "${CMAKE_BINARY_DIR}/publication-roots/geninc")

    _aros_generate_module_support(_root_probe
        TARGET tiff
        MMAKE_ID publication-root-probe
        DIRECTORY "${_probe_module_dir}"
        MODTYPE library)
    if(NOT TARGET publication-root-probe-includes-generated)
        message(FATAL_ERROR "public-root probe lacks its generated includes target")
    endif()
    list(LENGTH _root_probe_HEADERS _actual_header_count)
    set(_unique_headers "${_root_probe_HEADERS}")
    list(REMOVE_DUPLICATES _unique_headers)
    list(LENGTH _unique_headers _unique_header_count)
    if(NOT _actual_header_count EQUAL 10 OR
       NOT _unique_header_count EQUAL _actual_header_count)
        message(FATAL_ERROR
            "public-root probe expected ten unique headers across two roots, got "
            "${_actual_header_count} outputs and ${_unique_header_count} unique")
    endif()

    set(_expected_public_headers "")
    foreach(_root "${_probe_public_root}" "${AROS_GENINC_DIR}")
        foreach(_relative
                "clib/tiff_protos.h" "inline/tiff.h" "defines/tiff.h"
                "defines/tiff_LVO.h" "proto/tiff.h")
            list(APPEND _expected_public_headers "${_root}/${_relative}")
        endforeach()
    endforeach()
    set(_sorted_expected "${_expected_public_headers}")
    set(_sorted_actual "${_root_probe_HEADERS}")
    list(SORT _sorted_expected)
    list(SORT _sorted_actual)
    if(NOT _sorted_actual STREQUAL _sorted_expected)
        message(FATAL_ERROR
            "public-root probe published unexpected paths: ${_root_probe_HEADERS}")
    endif()
    return()
endif()

# Keep archive-selection counterprobes in this fixture project so they load the
# same production AROS.cmake as the main genmodule ABI test. The driver starts
# each mode in a fresh build tree: generated headers and CMake archive claims
# therefore cannot hide collisions between cases.
if(AROS_ARCHIVE_PROBE)
    set(_tiff_dir "${AROS_SOURCE_DIR}/workbench/libs/tiff")
    set(_tiff_source "${_tiff_dir}/tif_aros.c")
    set(_tiff_config "${_tiff_dir}/tiff.conf")
    if(NOT EXISTS "${_tiff_source}" OR NOT EXISTS "${_tiff_config}")
        message(FATAL_ERROR "archive probe requires the source-owned TIFF module")
    endif()

    if(AROS_ARCHIVE_PROBE MATCHES "^explicit-name-(distinct|case-only)$")
        set(_name_kind "${CMAKE_MATCH_1}")
        aros_test_bind_native_build_validation("${AROS_SOURCE_DIR}"
            "${CMAKE_CURRENT_BINARY_DIR}/native-build-contract-test.json")
        if(_name_kind STREQUAL "distinct")
            set(_explicit_name archive-probe-explicit)
        else()
            set(_explicit_name TIFF)
        endif()
        aros_add_library(
            TARGET tiff
            MMAKE_ID archive-explicit
            DIRECTORY "${_tiff_dir}"
            GENMODULE_LINKLIBS
            LINKLIB_NAME "${_explicit_name}"
            SOURCES "${_tiff_source}")
        foreach(_suffix "" "-rel")
            if(_suffix STREQUAL "-rel")
                set(_archive_suffix _rel)
            else()
                set(_archive_suffix "")
            endif()
            get_target_property(_primary_name "archive-explicit-linklib${_suffix}"
                OUTPUT_NAME)
            if(NOT _primary_name STREQUAL "${_explicit_name}${_archive_suffix}")
                message(FATAL_ERROR
                    "explicit LINKLIB_NAME is not the primary output: ${_primary_name}")
            endif()
        endforeach()
        return()
    endif()

    if(AROS_ARCHIVE_PROBE MATCHES
       "^reject-(no-contract|genmodule-only)-(all|normal|relative)$")
        set(_rejection_kind "${CMAKE_MATCH_1}")
        set(_archive_kind "${CMAKE_MATCH_2}")
        if(_archive_kind STREQUAL "all")
            set(_archive_flags NO_CLIENT_ARCHIVES)
        elseif(_archive_kind STREQUAL "normal")
            set(_archive_flags NO_NORMAL_CLIENT_ARCHIVE)
        else()
            set(_archive_flags NO_RELATIVE_CLIENT_ARCHIVE)
        endif()

        if(_rejection_kind STREQUAL "no-contract")
            aros_test_clear_native_build_validation()
            aros_add_library(
                TARGET tiff
                MMAKE_ID archive-rejection
                DIRECTORY "${_tiff_dir}"
                GENMODULE_LINKLIBS
                SOURCES "${_tiff_source}"
                ${_archive_flags})
        else()
            aros_test_bind_native_build_validation("${AROS_SOURCE_DIR}"
                "${CMAKE_CURRENT_BINARY_DIR}/native-build-contract-test.json")
            aros_add_library(
                TARGET tiff
                MMAKE_ID archive-rejection
                DIRECTORY "${_tiff_dir}"
                GENMODULE_ONLY
                ${_archive_flags})
        endif()
        message(FATAL_ERROR
            "${AROS_ARCHIVE_PROBE} unexpectedly accepted an unsupported archive mode")
    endif()

    if(AROS_ARCHIVE_PROBE MATCHES
       "^collision-(primary|relative|alias|alias-relative)-(client-first|implementation-first)$")
        set(_collision_kind "${CMAKE_MATCH_1}")
        set(_collision_order "${CMAKE_MATCH_2}")
        aros_test_bind_native_build_validation("${AROS_SOURCE_DIR}"
            "${CMAKE_CURRENT_BINARY_DIR}/native-build-contract-test.json")
        set(_client_linklib_name "")
        if(_collision_kind STREQUAL "primary")
            set(_implementation_name tiff)
            set(_collision_archive libtiff.a)
        elseif(_collision_kind STREQUAL "relative")
            set(_implementation_name tiff_rel)
            set(_collision_archive libtiff_rel.a)
        elseif(_collision_kind STREQUAL "alias")
            set(_client_linklib_name archive-probe-alias)
            set(_implementation_name "${_client_linklib_name}")
            set(_collision_archive "lib${_client_linklib_name}.a")
        else()
            set(_client_linklib_name archive-probe-alias)
            set(_implementation_name "${_client_linklib_name}_rel")
            set(_collision_archive "lib${_client_linklib_name}_rel.a")
        endif()

        set(_client_arguments
            TARGET tiff
            MMAKE_ID archive-client
            DIRECTORY "${_tiff_dir}"
            GENMODULE_LINKLIBS
            SOURCES "${_tiff_source}")
        if(_client_linklib_name)
            list(APPEND _client_arguments LINKLIB_NAME "${_client_linklib_name}")
        endif()
        set(_implementation_arguments
            TARGET "${_implementation_name}"
            MMAKE_ID archive-implementation
            DIRECTORY "${_tiff_dir}"
            CANONICAL_OUTPUT
            SOURCES "${_tiff_source}")

        if(_collision_order STREQUAL "client-first")
            aros_add_library(${_client_arguments})
            aros_add_linklib(${_implementation_arguments})
        else()
            aros_add_linklib(${_implementation_arguments})
            aros_add_library(${_client_arguments})
        endif()
        message(FATAL_ERROR
            "${AROS_ARCHIVE_PROBE} failed to reject canonical archive collision ${_collision_archive}")
    endif()

    if(AROS_ARCHIVE_PROBE MATCHES
       "^positive-(all|normal-suppressed|relative-suppressed)$")
        set(_suppression_kind "${CMAKE_MATCH_1}")
        aros_test_bind_native_build_validation("${AROS_SOURCE_DIR}"
            "${CMAKE_CURRENT_BINARY_DIR}/native-build-contract-test.json")
        aros_genmodule_writefiles_manifest(_archive_manifest
            CONFIG "${_tiff_config}"
            MODULE tiff
            MODTYPE library
            GEN_DIR "${CMAKE_CURRENT_BINARY_DIR}/manifest/gen"
            STUB_DIR "${CMAKE_CURRENT_BINARY_DIR}/manifest/linklib")
        if(NOT _archive_manifest_HAS_REL_LINKLIB OR
           NOT _archive_manifest_HAS_FUNCTIONS OR
           NOT _archive_manifest_HAS_INCLUDES)
            message(FATAL_ERROR
                "TIFF fixture no longer owns relative linklib, function and include outputs")
        endif()

        set(_archive_arguments
            TARGET tiff
            MMAKE_ID archive-native
            DIRECTORY "${_tiff_dir}"
            GENMODULE_LINKLIBS
            LINKLIB_NAME archive-probe-alias
            SOURCES "${_tiff_source}")
        if(_suppression_kind STREQUAL "all")
            list(APPEND _archive_arguments NO_CLIENT_ARCHIVES)
        elseif(_suppression_kind STREQUAL "normal-suppressed")
            list(APPEND _archive_arguments NO_NORMAL_CLIENT_ARCHIVE)
        else()
            list(APPEND _archive_arguments NO_RELATIVE_CLIENT_ARCHIVE)
        endif()
        aros_add_library(${_archive_arguments})

        foreach(_required_target archive-native archive-native-includes
                archive-native-fd archive-native-genmodfiles)
            if(NOT TARGET "${_required_target}")
                message(FATAL_ERROR
                    "${_suppression_kind} suppression lost ABI/runtime target ${_required_target}")
            endif()
        endforeach()
        get_target_property(_runtime_dependencies archive-native
            MANUALLY_ADDED_DEPENDENCIES)
        foreach(_required_dependency archive-native-includes archive-native-fd)
            if(NOT _required_dependency IN_LIST _runtime_dependencies)
                message(FATAL_ERROR
                    "runtime lost ${_required_dependency} dependency under ${_suppression_kind} suppression: ${_runtime_dependencies}")
            endif()
        endforeach()
        set(_absent_archive_dependencies "")
        set(_required_archive_dependencies "")
        if(_suppression_kind STREQUAL "all")
            set(_absent_archive_dependencies
                archive-native-linklib archive-native-linklib-rel)
        elseif(_suppression_kind STREQUAL "normal-suppressed")
            set(_absent_archive_dependencies archive-native-linklib)
            set(_required_archive_dependencies archive-native-linklib-rel)
        else()
            set(_absent_archive_dependencies archive-native-linklib-rel)
            set(_required_archive_dependencies archive-native-linklib)
        endif()
        foreach(_absent_dependency IN LISTS _absent_archive_dependencies)
            if(_absent_dependency IN_LIST _runtime_dependencies)
                message(FATAL_ERROR
                    "runtime retains suppressed archive dependency ${_absent_dependency}")
            endif()
        endforeach()
        foreach(_required_dependency IN LISTS _required_archive_dependencies)
            if(NOT _required_dependency IN_LIST _runtime_dependencies)
                message(FATAL_ERROR
                    "runtime lost enabled archive dependency ${_required_dependency}")
            endif()
        endforeach()

        if(_suppression_kind STREQUAL "all" OR
           _suppression_kind STREQUAL "normal-suppressed")
            if(TARGET archive-native-linklib OR TARGET linklibs-tiff OR
               TARGET linklibs-archive-probe-alias)
                message(FATAL_ERROR
                    "normal client archive or alias exists under ${_suppression_kind} suppression")
            endif()
        else()
            foreach(_required_target archive-native-linklib linklibs-tiff
                    linklibs-archive-probe-alias)
                if(NOT TARGET "${_required_target}")
                    message(FATAL_ERROR
                        "normal client archive or alias ${_required_target} missing under relative-only suppression")
                endif()
            endforeach()
        endif()

        if(_suppression_kind STREQUAL "all" OR
           _suppression_kind STREQUAL "relative-suppressed")
            if(TARGET archive-native-linklib-rel OR TARGET linklibs-tiff_rel OR
               TARGET linklibs-archive-probe-alias_rel)
                message(FATAL_ERROR
                    "relative client archive or alias exists under ${_suppression_kind} suppression")
            endif()
        else()
            foreach(_required_target archive-native-linklib-rel linklibs-tiff_rel
                    linklibs-archive-probe-alias_rel)
                if(NOT TARGET "${_required_target}")
                    message(FATAL_ERROR
                        "relative client archive or alias ${_required_target} missing under normal-only suppression")
                endif()
            endforeach()
        endif()
        return()
    endif()

    if(AROS_ARCHIVE_PROBE STREQUAL "positive-consumer-suppressed")
        aros_add_library(
            TARGET tiff
            MMAKE_ID consumer-native
            DIRECTORY "${_tiff_dir}"
            GENMODULE_LINKLIBS
            SOURCES "${_tiff_source}"
            NO_CLIENT_ARCHIVES)
        foreach(_required_target consumer-native consumer-native-includes
                consumer-native-fd consumer-native-genmodfiles)
            if(NOT TARGET "${_required_target}")
                message(FATAL_ERROR
                    "consumer NO_CLIENT_ARCHIVES lost ABI/runtime target ${_required_target}")
            endif()
        endforeach()
        get_target_property(_runtime_dependencies consumer-native
            MANUALLY_ADDED_DEPENDENCIES)
        foreach(_required_dependency consumer-native-includes consumer-native-fd)
            if(NOT _required_dependency IN_LIST _runtime_dependencies)
                message(FATAL_ERROR
                    "consumer runtime lost ${_required_dependency}: ${_runtime_dependencies}")
            endif()
        endforeach()
        foreach(_absent_target consumer-native-linklib consumer-native-linklib-rel)
            if(TARGET "${_absent_target}")
                message(FATAL_ERROR
                    "consumer NO_CLIENT_ARCHIVES retained ${_absent_target}")
            endif()
        endforeach()
        return()
    endif()

    # make.tmpl keeps a suffixed module's client archives in a private
    # directory. There they cannot take the place of an SDK archive whose
    # name differs only in case; in the SDK directory they must be refused.
    if(AROS_ARCHIVE_PROBE MATCHES "^case-variant-(private|sdk)$")
        set(_client_location "${CMAKE_MATCH_1}")
        aros_test_bind_native_build_validation("${AROS_SOURCE_DIR}"
            "${CMAKE_CURRENT_BINARY_DIR}/native-build-contract-test.json")
        set(_private_dir "${CMAKE_BINARY_DIR}/gen/plugins")
        set(_client_arguments
            TARGET tiff
            MMAKE_ID archive-plugin
            DIRECTORY "${_tiff_dir}"
            GENMODULE_LINKLIBS
            SOURCES "${_tiff_source}")
        if(_client_location STREQUAL "private")
            list(APPEND _client_arguments OUTPUT_DIR "${_private_dir}")
        endif()
        aros_add_library(${_client_arguments})
        aros_add_linklib(
            TARGET TIFF
            MMAKE_ID archive-sdk
            DIRECTORY "${_tiff_dir}"
            CANONICAL_OUTPUT
            SOURCES "${_tiff_source}")
        if(_client_location STREQUAL "sdk")
            message(FATAL_ERROR "case-variant client archive in the SDK was accepted")
        endif()
        foreach(_client_target archive-plugin-linklib archive-plugin-linklib-rel)
            get_target_property(_client_dir "${_client_target}" ARCHIVE_OUTPUT_DIRECTORY)
            if(NOT _client_dir STREQUAL _private_dir)
                message(FATAL_ERROR
                    "${_client_target} left its private directory: ${_client_dir}")
            endif()
            aros_linklib_archive_path("${_client_target}" _client_archive)
            if(NOT _client_archive MATCHES "^${_private_dir}/")
                message(FATAL_ERROR
                    "${_client_target} archive path is not private: ${_client_archive}")
            endif()
        endforeach()
        return()
    endif()

    # dxtn.conf spells its list "## begin functionlist", so genmodule sees no
    # functions and writes no FD, in Make as here. The ABI form must still
    # publish its headers and client archive.
    if(AROS_ARCHIVE_PROBE STREQUAL "abi-without-fd")
        aros_test_bind_native_build_validation("${AROS_SOURCE_DIR}"
            "${CMAKE_CURRENT_BINARY_DIR}/native-build-contract-test.json")
        aros_add_module_abi(
            TARGET dxtn
            MMAKE_ID abi-no-fd
            MODTYPE library
            DIRECTORY "${AROS_SOURCE_DIR}/workbench/libs/dxtn")
        foreach(_required_target abi-no-fd abi-no-fd-includes abi-no-fd-linklib)
            if(NOT TARGET "${_required_target}")
                message(FATAL_ERROR "ABI module without FD lost ${_required_target}")
            endif()
        endforeach()
        if(TARGET abi-no-fd-fd)
            message(FATAL_ERROR "ABI module without FD declared an FD endpoint")
        endif()
        return()
    endif()

    message(FATAL_ERROR "unknown genmodule archive probe: ${AROS_ARCHIVE_PROBE}")
endif()

set(_includes_stamp "${CMAKE_CURRENT_BINARY_DIR}/includes.stamp")
set(_fd_stamp "${CMAKE_CURRENT_BINARY_DIR}/fd.stamp")
add_custom_command(
    OUTPUT "${_includes_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_includes_stamp}"
    VERBATIM)
add_custom_command(
    OUTPUT "${_fd_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_fd_stamp}"
    VERBATIM)
add_custom_target(real-includes DEPENDS "${_includes_stamp}")
add_custom_target(real-fd DEPENDS "${_fd_stamp}")

_aros_bind_genmodule_abi_targets(probe real-includes real-fd)

get_property(_includes_deps TARGET probe-includes
    PROPERTY MANUALLY_ADDED_DEPENDENCIES)
foreach(_required real-includes real-fd)
    if(NOT _required IN_LIST _includes_deps)
        message(FATAL_ERROR
            "probe-includes does not depend on ${_required}: ${_includes_deps}")
    endif()
endforeach()
get_property(_fd_deps TARGET probe-fd PROPERTY MANUALLY_ADDED_DEPENDENCIES)
if(NOT "real-fd" IN_LIST _fd_deps)
    message(FATAL_ERROR "probe-fd does not depend on real-fd: ${_fd_deps}")
endif()

set(_miami_dir
    "${AROS_SOURCE_DIR}/workbench/network/stacks/AROSTCP/MUI.MiamiPanel")
set(_miami_config "${_miami_dir}/MUI.MiamiPanel.conf")
aros_set_module_config(probe-mui "${_miami_config}")
aros_add_library(
    TARGET MUI
    MMAKE_ID probe-mui
    DIRECTORY "${_miami_dir}"
    GENMODULE_LINKLIBS
    SOURCES "${_miami_dir}/muimiamipanel_init.c")

if(NOT TARGET probe-mui-includes OR NOT TARGET includes-MUI)
    message(FATAL_ERROR "MUI.MiamiPanel noincludes did not retain its includes aliases")
endif()
if(TARGET probe-mui-fd OR TARGET probe-mui-fd-generated)
    message(FATAL_ERROR "MUI.MiamiPanel noincludes incorrectly created an FD target")
endif()

get_target_property(_mui_archive_sources probe-mui-linklib SOURCES)
list(FILTER _mui_archive_sources INCLUDE REGEX "_getlibbase\\.c$")
list(LENGTH _mui_archive_sources _mui_getlibbase_count)
if(NOT _mui_getlibbase_count EQUAL 1)
    message(FATAL_ERROR
        "MUI.MiamiPanel noincludes lost its normal getlibbase archive source: "
        "${_mui_archive_sources}")
endif()
list(GET _mui_archive_sources 0 _mui_getlibbase_source)
if(NOT _mui_getlibbase_source MATCHES "/linklib/MUI_getlibbase\\.c$")
    message(FATAL_ERROR
        "MUI.MiamiPanel archive source is not the normal getlibbase output: "
        "${_mui_getlibbase_source}")
endif()
get_source_file_property(_mui_getlibbase_generated
    "${_mui_getlibbase_source}" GENERATED)
if(NOT _mui_getlibbase_generated)
    message(FATAL_ERROR
        "MUI.MiamiPanel getlibbase was not marked as a generated archive source")
endif()
file(WRITE "${CMAKE_BINARY_DIR}/mui-getlibbase-source.txt"
    "${_mui_getlibbase_source}")
