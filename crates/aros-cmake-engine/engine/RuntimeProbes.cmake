# Opt-in guest evidence for the currently qualified PC LLVM11/Mesa26 lane.
# These are tools-owned probes, not additions to the AROS source checkout.
option(AROS_LLVMPIPE_RUNTIME_PROBE "Build the PC llvmpipe shader probe and MCJIT audit wrapper" OFF)
if(DEFINED ENV{AROS_LLVMPIPE_RUNTIME_PROBE})
    if("$ENV{AROS_LLVMPIPE_RUNTIME_PROBE}" STREQUAL "1")
        set(AROS_LLVMPIPE_RUNTIME_PROBE ON)
    elseif("$ENV{AROS_LLVMPIPE_RUNTIME_PROBE}" STREQUAL "0")
        set(AROS_LLVMPIPE_RUNTIME_PROBE OFF CACHE BOOL
            "Build the PC llvmpipe shader probe and MCJIT audit wrapper" FORCE)
    else()
        message(FATAL_ERROR "AROS_LLVMPIPE_RUNTIME_PROBE must be 0 or 1")
    endif()
endif()

if(AROS_LLVMPIPE_RUNTIME_PROBE)
    if(NOT AROS_TARGET_CPU STREQUAL "x86_64" OR
       NOT AROS_TARGET_PLATFORM STREQUAL "pc" OR
       NOT AROS_MESA_VERSION STREQUAL "26.0.0" OR
       NOT TARGET hidd-llvmpipe OR NOT TARGET workbench-libs-gl-linklib OR
       NOT TARGET workbench-libs-gl OR
       NOT TARGET mesa3dgl-library OR
       NOT TARGET workbench-libs-llvm-external-LLVM OR NOT AROS_LLD_BIN)
        message(FATAL_ERROR "llvmpipe runtime probe requires the native PC LLVM11/Mesa26 producers")
    endif()
    # --wrap affects only the audit build. The wrapper returns the genuine
    # MCJIT pointer unchanged; it logs no fabricated success or execution.
    target_sources(hidd-llvmpipe PRIVATE
        "${CMAKE_CURRENT_LIST_DIR}/tests/runtime/llvmpipe-jit-audit.c")
    target_link_options(hidd-llvmpipe PRIVATE --wrap=LLVMGetPointerToGlobal)
    aros_add_program(
        TARGET llvmpipe-jit
        MMAKE_ID tools-test-llvmpipe-jit
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}/tests/runtime"
        INSTALL_DIR "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/graphics/gl"
        SOURCES llvmpipe-jit
        LIBS workbench-libs-gl-linklib)
    if(NOT TARGET tools-test-llvmpipe-jit)
        message(FATAL_ERROR "llvmpipe guest probe did not materialize")
    endif()
    add_dependencies(tools-test-llvmpipe-jit hidd-llvmpipe workbench-libs-gl)
    aros_add_program(
        TARGET llvmpipe-jit-runner
        MMAKE_ID tools-test-llvmpipe-jit-runner
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}/tests/runtime"
        INSTALL_DIR "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/graphics/gl"
        SOURCES llvmpipe-jit-runner)
    add_dependencies(tools-test-llvmpipe-jit-runner tools-test-llvmpipe-jit)
    message(STATUS "llvmpipe runtime probe enabled: MCJIT address logging is instrumentation, not a PASS gate")
endif()
