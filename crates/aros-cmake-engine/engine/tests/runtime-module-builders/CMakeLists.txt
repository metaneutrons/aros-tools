cmake_minimum_required(VERSION 3.22)
project(RuntimeModuleBuilderOutputs C)

list(INSERT CMAKE_MODULE_PATH 0 "${CMAKE_CURRENT_LIST_DIR}/cmake")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_LLD_BIN FALSE)
include("${AROS_TEST_ENGINE_DIR}/AROS.cmake")

set(_module_dir "${AROS_SOURCE_DIR}/workbench/libs/version")
set(_runtime_id runtime-owner)
if(TEST_BUILDER STREQUAL "library")
    aros_set_module_config(runtime-owner "${_module_dir}/version.conf")
    aros_add_library(
        TARGET version
        MMAKE_ID runtime-owner
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        INSTALL_DIR "${AROS_RESOURCES_DIR}"
        MODSUFFIX resource
        SOURCES "${CMAKE_CURRENT_LIST_DIR}/module.c")
elseif(TEST_BUILDER STREQUAL "genmodule-only")
    aros_add_library(
        GENMODULE_ONLY
        TARGET version
        MMAKE_ID runtime-owner
        DIRECTORY "${_module_dir}"
        INSTALL_DIR "${AROS_RESOURCES_DIR}"
        MODSUFFIX resource)
elseif(TEST_BUILDER STREQUAL "device")
    set(_runtime_id version)
    aros_set_module_config(version "${_module_dir}/version.conf")
    aros_add_device(
        TARGET version
        MMAKE_ID version
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        MODSUFFIX resource
        SOURCES "${CMAKE_CURRENT_LIST_DIR}/module.c")
elseif(TEST_BUILDER STREQUAL "resource")
    set(_runtime_id version)
    aros_set_module_config(version "${_module_dir}/version.conf")
    aros_add_resource(
        TARGET version
        MMAKE_ID version
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        SOURCES "${CMAKE_CURRENT_LIST_DIR}/module.c")
else()
    message(FATAL_ERROR "unknown runtime builder: ${TEST_BUILDER}")
endif()

if(TEST_MANUAL_BUILDER STREQUAL "simple")
    aros_add_module_simple(
        TARGET version
        MMAKE_ID manual-owner
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        MODTYPE resource
        SOURCES "${CMAKE_CURRENT_LIST_DIR}/module.c")
elseif(TEST_MANUAL_BUILDER STREQUAL "custom")
    aros_add_custom_target(
        TARGET version
        MMAKE_ID manual-owner
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        MODTYPE resource
        SOURCES "${CMAKE_CURRENT_LIST_DIR}/module.c")
else()
    message(FATAL_ERROR "unknown manual builder: ${TEST_MANUAL_BUILDER}")
endif()

if(NOT TARGET "${_runtime_id}" OR NOT TARGET manual-owner)
    message(FATAL_ERROR "runtime builder did not create both module targets")
endif()
file(GENERATE OUTPUT "${CMAKE_BINARY_DIR}/runtime-owner.path"
    CONTENT "$<TARGET_FILE:${_runtime_id}>")
file(GENERATE OUTPUT "${CMAKE_BINARY_DIR}/manual-owner.path"
    CONTENT "$<TARGET_FILE:manual-owner>")

add_custom_target(AROS)
if(TEST_SELECTION STREQUAL "runtime" OR TEST_SELECTION STREQUAL "both")
    add_dependencies(AROS "${_runtime_id}")
endif()
if(TEST_SELECTION STREQUAL "manual" OR TEST_SELECTION STREQUAL "both")
    add_dependencies(AROS manual-owner)
endif()
if(NOT TEST_SELECTION MATCHES "^(runtime|manual|both)$")
    message(FATAL_ERROR "unknown runtime selection: ${TEST_SELECTION}")
endif()
aros_limit_all_to_metamake_root(AROS)
