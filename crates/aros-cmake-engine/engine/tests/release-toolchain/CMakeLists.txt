cmake_minimum_required(VERSION 3.22)
project(release_toolchain_fixture NONE)

include("${CMAKE_CURRENT_LIST_DIR}/../../ToolchainIdentity.cmake")
aros_lock_build_tree_toolchain()

foreach(_required IN ITEMS
        AROS_CROSS_TOOLCHAIN_ROOT AROS_TARGET_PROFILE AROS_TARGET_TRIPLE
        AROS_CROSS_TOOLCHAIN_RELEASE_ID AROS_CROSS_TOOLCHAIN_TREE_SHA256
        AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES
        AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE
        CMAKE_C_COMPILER CMAKE_CXX_COMPILER CMAKE_AR CMAKE_RANLIB AROS_LLD_BIN)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "release toolchain did not define ${_required}")
    endif()
endforeach()
foreach(_platform_variable IN ITEMS
        AROS_CROSS_TOOLCHAIN_ROOT AROS_TARGET_CPU AROS_TARGET_PLATFORM)
    if(NOT _platform_variable IN_LIST CMAKE_TRY_COMPILE_PLATFORM_VARIABLES)
        message(FATAL_ERROR
            "release toolchain does not propagate ${_platform_variable} into try_compile")
    endif()
endforeach()
if(NOT AROS_TARGET_PROFILE STREQUAL EXPECTED_PROFILE OR
   NOT AROS_TARGET_TRIPLE STREQUAL EXPECTED_TRIPLE)
    message(FATAL_ERROR
        "release toolchain selected ${AROS_TARGET_PROFILE}/${AROS_TARGET_TRIPLE}, "
        "expected ${EXPECTED_PROFILE}/${EXPECTED_TRIPLE}")
endif()
if(NOT CMAKE_C_COMPILER STREQUAL "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/clang" OR
   NOT CMAKE_CXX_COMPILER STREQUAL "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/clang++" OR
   NOT CMAKE_AR STREQUAL "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/llvm-ar")
    message(FATAL_ERROR "release toolchain did not select prefix-owned tools")
endif()

set(_expected_cxx_runtime_libraries
    "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libc++.a"
    "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libc++abi.a"
    "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libunwind.a"
    "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/clang/11.0.0/lib/aros/libclang_rt.builtins-${EXPECTED_BUILTINS}.a")
if(NOT AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES STREQUAL
       _expected_cxx_runtime_libraries OR
   NOT AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE STREQUAL
       "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/clang/11.0.0/lib/aros/libclang_rt.builtins-${EXPECTED_BUILTINS}.a")
    message(FATAL_ERROR
        "release toolchain did not publish the exact prefix-owned C++ runtime")
endif()
