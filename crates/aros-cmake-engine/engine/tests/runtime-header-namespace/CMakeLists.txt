cmake_minimum_required(VERSION 3.22)
set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
project(RuntimeHeaderNamespaceFixture LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT TEST_AROS_TOOLCHAIN MATCHES "^(gnu|llvm)$")
    message(FATAL_ERROR "fixture needs an explicit GNU or LLVM family")
endif()
set(AROS_TOOLCHAIN "${TEST_AROS_TOOLCHAIN}" CACHE STRING "" FORCE)
set(AROS_TARGET_CPU riscv32)
set(AROS_TARGET_PLATFORM pc)
set(AROS_SOURCE_DIR "${TEST_SOURCE_ROOT}")
list(INSERT CMAKE_MODULE_PATH 0 "${CMAKE_CURRENT_LIST_DIR}/cmake")

file(MAKE_DIRECTORY
    "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc"
    "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc"
    "${CMAKE_BINARY_DIR}/GENINCDIR"
    "${CMAKE_BINARY_DIR}/gen/module")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc/namespace_order.h"
    "#define AROS_NAMESPACE_ID 1\n")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc/namespace_order.h"
    "#define AROS_NAMESPACE_ID 2\n")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/namespace_order.h"
    "#define AROS_NAMESPACE_ID 3\n")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc/generated_order.h"
    "#define AROS_GENERATED_ID 1\n")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc/generated_order.h"
    "#define AROS_GENERATED_ID 2\n")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/generated_order.h"
    "#define AROS_GENERATED_ID 3\n")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc/private_order.h"
    "#define AROS_PRIVATE_ID 1\n")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc/private_order.h"
    "#define AROS_PRIVATE_ID 2\n")
file(WRITE "${CMAKE_BINARY_DIR}/SDK/include/private_order.h"
    "#define AROS_PRIVATE_ID 3\n")
file(WRITE "${CMAKE_BINARY_DIR}/GENINCDIR/generated_order.h"
    "#define AROS_GENERATED_ID 4\n")
file(WRITE "${CMAKE_BINARY_DIR}/gen/module/private_order.h"
    "#define AROS_PRIVATE_ID 5\n")

# The namespace fixture builds only object files. Give the engine an owned
# sentinel so configure never resolves a host aros-collect from PATH; invoking
# this collector is a fixture failure.
set(_fixture_collector "${CMAKE_BINARY_DIR}/inert-aros-collect")
file(WRITE "${_fixture_collector}" "#!/bin/sh\nexit 97\n")
file(CHMOD "${_fixture_collector}"
    PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE
                GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
set(AROS_COLLECT_BIN "${_fixture_collector}")

include("${TEST_ENGINE_DIR}/AROS.cmake")
if(NOT AROS_COLLECT_BIN STREQUAL _fixture_collector)
    message(FATAL_ERROR
        "namespace fixture did not retain its explicitly owned collector sentinel")
endif()

set(_probe_source "${CMAKE_CURRENT_LIST_DIR}/probe.c")
set(_probe_cpp_source "${CMAKE_CURRENT_LIST_DIR}/probe.cpp")
set(_module_dir "${AROS_SOURCE_DIR}/module")

function(_add_namespace_probe target source expected_namespace expected_generated)
    add_library("${target}" OBJECT "${source}")
    target_compile_definitions("${target}" PRIVATE
        "EXPECTED_NAMESPACE_ID=${expected_namespace}"
        "EXPECTED_GENERATED_ID=${expected_generated}"
        EXPECTED_PRIVATE_ID=5)
    aros_apply_includes("${target}" MODULE_DIR "${_module_dir}")
    if(target MATCHES "^noposixc")
        aros_apply_flags("${target}" COMPILE_OPTIONS -noposixc)
    elseif(target MATCHES "^explicit-posixc")
        aros_apply_flags("${target}" COMPILE_OPTIONS -noposixc)
        aros_apply_includes("${target}"
            INCLUDES "${AROS_SDK_INCLUDE_DIR}/aros/posixc")
    endif()
endfunction()

_add_namespace_probe(default-c "${_probe_source}" 1 4)
_add_namespace_probe(default-cxx "${_probe_cpp_source}" 1 4)
_add_namespace_probe(noposixc-c "${_probe_source}" 2 4)
_add_namespace_probe(noposixc-cxx "${_probe_cpp_source}" 2 4)
if(TEST_AROS_TOOLCHAIN STREQUAL "gnu")
    # GCC orders normal generated roots before SYSTEM namespaces regardless
    # of where its AROS specs place the same namespace directories.
    set(_explicit_generated_id 4)
else()
    # LLVM retains the existing declaration-local BEFORE include order.
    set(_explicit_generated_id 1)
endif()
_add_namespace_probe(explicit-posixc-c
    "${_probe_source}" 1 "${_explicit_generated_id}")
_add_namespace_probe(explicit-posixc-cxx
    "${_probe_cpp_source}" 1 "${_explicit_generated_id}")
