cmake_minimum_required(VERSION 3.22)
project(RuntimeProbeFixture C)

if(NOT DEFINED RUNTIME_PROBES_MODULE OR
   NOT EXISTS "${RUNTIME_PROBES_MODULE}")
    message(FATAL_ERROR "RUNTIME_PROBES_MODULE must name RuntimeProbes.cmake")
endif()

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_MESA_VERSION 26.0.0)
set(AROS_LLD_BIN "${CMAKE_CURRENT_BINARY_DIR}/fake-ld.lld")
set(AROS_BUILD_DIR "${CMAKE_CURRENT_BINARY_DIR}")

if(RUNTIME_PROBE_CASE STREQUAL "default-off")
    unset(AROS_LLVMPIPE_RUNTIME_PROBE CACHE)
    unset(AROS_LLVMPIPE_RUNTIME_PROBE)
    include("${RUNTIME_PROBES_MODULE}")
    if(AROS_LLVMPIPE_RUNTIME_PROBE OR TARGET tools-test-llvmpipe-jit)
        message(FATAL_ERROR "llvmpipe runtime probe was not opt-in by default")
    endif()
    return()
endif()

if(RUNTIME_PROBE_CASE STREQUAL "reset-off")
    set(AROS_LLVMPIPE_RUNTIME_PROBE ON CACHE BOOL "prior probe configuration" FORCE)
    include("${RUNTIME_PROBES_MODULE}")
    if(AROS_LLVMPIPE_RUNTIME_PROBE OR TARGET tools-test-llvmpipe-jit)
        message(FATAL_ERROR "explicit environment 0 did not disable the cached probe")
    endif()
    return()
endif()

if(RUNTIME_PROBE_CASE STREQUAL "wrong-cpu")
    set(AROS_TARGET_CPU aarch64)
elseif(RUNTIME_PROBE_CASE STREQUAL "wrong-platform")
    set(AROS_TARGET_PLATFORM amiga)
elseif(RUNTIME_PROBE_CASE STREQUAL "wrong-mesa")
    set(AROS_MESA_VERSION 25.0.0)
endif()

if(NOT RUNTIME_PROBE_CASE STREQUAL "missing-hidd")
    add_library(hidd-llvmpipe STATIC dummy.c)
endif()
if(NOT RUNTIME_PROBE_CASE STREQUAL "missing-gl-linklib")
    add_library(workbench-libs-gl-linklib INTERFACE)
endif()
add_custom_target(workbench-libs-gl)
if(NOT RUNTIME_PROBE_CASE STREQUAL "missing-gl-provider")
    add_custom_target(mesa3dgl-library)
endif()
if(NOT RUNTIME_PROBE_CASE STREQUAL "missing-llvm-producer")
    add_library(workbench-libs-llvm-external-LLVM INTERFACE)
endif()
if(RUNTIME_PROBE_CASE STREQUAL "missing-lld")
    set(AROS_LLD_BIN "")
endif()

function(aros_add_program)
    set(one_value_args TARGET MMAKE_ID DIRECTORY INSTALL_DIR)
    set(multi_value_args SOURCES LIBS)
    cmake_parse_arguments(PARSE_ARGV 0 PROGRAM
        "" "${one_value_args}" "${multi_value_args}")
    if(PROGRAM_UNPARSED_ARGUMENTS OR PROGRAM_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "fixture received malformed aros_add_program arguments")
    endif()
    if(TARGET "${PROGRAM_MMAKE_ID}")
        message(FATAL_ERROR "fixture program target already exists")
    endif()
    add_custom_target("${PROGRAM_MMAKE_ID}")
    set_property(TARGET "${PROGRAM_MMAKE_ID}" PROPERTY
        RUNTIME_PROBE_PROGRAM_TARGET "${PROGRAM_TARGET}")
    set_property(TARGET "${PROGRAM_MMAKE_ID}" PROPERTY
        RUNTIME_PROBE_PROGRAM_DIRECTORY "${PROGRAM_DIRECTORY}")
    set_property(TARGET "${PROGRAM_MMAKE_ID}" PROPERTY
        RUNTIME_PROBE_PROGRAM_INSTALL_DIR "${PROGRAM_INSTALL_DIR}")
    set_property(TARGET "${PROGRAM_MMAKE_ID}" PROPERTY
        RUNTIME_PROBE_PROGRAM_SOURCES "${PROGRAM_SOURCES}")
    set_property(TARGET "${PROGRAM_MMAKE_ID}" PROPERTY
        RUNTIME_PROBE_PROGRAM_LIBS "${PROGRAM_LIBS}")
endfunction()

include("${RUNTIME_PROBES_MODULE}")

if(NOT RUNTIME_PROBE_CASE STREQUAL "enabled")
    return()
endif()

set(_audit_source
    "${CMAKE_CURRENT_LIST_DIR}/../runtime/llvmpipe-jit-audit.c")
cmake_path(NORMAL_PATH _audit_source)
get_filename_component(_runtime_probe_engine_dir
    "${RUNTIME_PROBES_MODULE}" DIRECTORY)
set(_runtime_probe_source_dir
    "${_runtime_probe_engine_dir}/tests/runtime")
get_target_property(_hidd_sources hidd-llvmpipe SOURCES)
if(NOT _audit_source IN_LIST _hidd_sources)
    message(FATAL_ERROR "llvmpipe audit source was not attached to hidd-llvmpipe")
endif()
get_target_property(_hidd_link_options hidd-llvmpipe LINK_OPTIONS)
if(NOT "--wrap=LLVMGetPointerToGlobal" IN_LIST _hidd_link_options)
    message(FATAL_ERROR "llvmpipe MCJIT audit linker wrap was not attached")
endif()
if(NOT TARGET tools-test-llvmpipe-jit)
    message(FATAL_ERROR "stub did not materialize the guest probe target")
endif()
if(NOT TARGET tools-test-llvmpipe-jit-runner)
    message(FATAL_ERROR "guest lifecycle supervisor was not materialized")
endif()
get_target_property(_runner_dependencies tools-test-llvmpipe-jit-runner
    MANUALLY_ADDED_DEPENDENCIES)
if(NOT "tools-test-llvmpipe-jit" IN_LIST _runner_dependencies)
    message(FATAL_ERROR "guest lifecycle supervisor lacks the probe dependency")
endif()
get_target_property(_probe_dependencies tools-test-llvmpipe-jit
    MANUALLY_ADDED_DEPENDENCIES)
if(NOT "hidd-llvmpipe" IN_LIST _probe_dependencies)
    message(FATAL_ERROR "guest probe target does not depend on hidd-llvmpipe")
endif()
if(NOT "workbench-libs-gl" IN_LIST _probe_dependencies)
    message(FATAL_ERROR "guest probe target does not depend on the GL runtime")
endif()
get_target_property(_program_target tools-test-llvmpipe-jit
    RUNTIME_PROBE_PROGRAM_TARGET)
get_target_property(_program_directory tools-test-llvmpipe-jit
    RUNTIME_PROBE_PROGRAM_DIRECTORY)
get_target_property(_program_install_dir tools-test-llvmpipe-jit
    RUNTIME_PROBE_PROGRAM_INSTALL_DIR)
get_target_property(_program_sources tools-test-llvmpipe-jit
    RUNTIME_PROBE_PROGRAM_SOURCES)
get_target_property(_program_libs tools-test-llvmpipe-jit
    RUNTIME_PROBE_PROGRAM_LIBS)
set(_expected_install_dir
    "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/graphics/gl")
if(NOT "${_program_target}" STREQUAL "llvmpipe-jit" OR
   NOT "${_program_directory}" STREQUAL "${_runtime_probe_source_dir}" OR
   NOT "${_program_install_dir}" STREQUAL "${_expected_install_dir}" OR
   NOT "${_program_sources}" STREQUAL "llvmpipe-jit" OR
   NOT "${_program_libs}" STREQUAL "workbench-libs-gl-linklib")
    message(FATAL_ERROR
        "llvmpipe guest program contract is wrong: target=${_program_target} "
        "directory=${_program_directory} install=${_program_install_dir} "
        "sources=${_program_sources} libs=${_program_libs}; expected install="
        "${_expected_install_dir} directory=${_runtime_probe_source_dir}")
endif()
