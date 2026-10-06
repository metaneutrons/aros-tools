cmake_minimum_required(VERSION 3.22)
project(gnu_release_toolchain_fixture NONE)

foreach(_required IN ITEMS
        AROS_TOOLCHAIN AROS_TARGET_CPU AROS_TARGET_PLATFORM AROS_TARGET_PROFILE
        AROS_CROSS_TOOLCHAIN_PROFILE EXPECTED_COMPILER_PROFILE
        AROS_TARGET_TRIPLE AROS_CROSS_TOOLCHAIN_ROOT AROS_CROSS_TOOLCHAIN_RELEASE_ID
        AROS_CROSS_TOOLCHAIN_TREE_SHA256 AROS_GNU_TARGET_COMPILE_OPTIONS
        AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE
        AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES CMAKE_C_COMPILER
        CMAKE_CXX_COMPILER CMAKE_ASM_COMPILER CMAKE_AR CMAKE_RANLIB CMAKE_STRIP
        CMAKE_NM CMAKE_OBJCOPY AROS_AS_BIN AROS_LINKER_BIN AROS_COLLECT_BIN)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "GNU release toolchain did not define ${_required}")
    endif()
endforeach()

if(NOT DEFINED EXPECTED_TOOL_LAYOUT_SCHEMA OR
   NOT EXPECTED_TOOL_LAYOUT_SCHEMA MATCHES "^aros-toolchain-tools-v[23]$")
    message(FATAL_ERROR "GNU release toolchain fixture lacks its expected layout schema")
endif()
if(EXPECTED_TOOL_LAYOUT_SCHEMA STREQUAL "aros-toolchain-tools-v3")
    if(NOT DEFINED CMAKE_OBJDUMP OR
       NOT CMAKE_OBJDUMP STREQUAL "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/objdump")
        message(FATAL_ERROR "GNU v3 objdump role was not selected from the prefix")
    endif()
elseif(DEFINED CMAKE_OBJDUMP AND NOT CMAKE_OBJDUMP STREQUAL "")
    message(FATAL_ERROR "GNU v2 layout retained an ambient CMAKE_OBJDUMP")
endif()

if(NOT AROS_TOOLCHAIN STREQUAL "gnu" OR
   NOT AROS_TARGET_PROFILE STREQUAL EXPECTED_PROFILE OR
   NOT AROS_CROSS_TOOLCHAIN_PROFILE STREQUAL EXPECTED_COMPILER_PROFILE OR
   NOT AROS_TARGET_PLATFORM STREQUAL EXPECTED_PLATFORM OR
   NOT AROS_TARGET_TRIPLE STREQUAL EXPECTED_TRIPLE)
    message(FATAL_ERROR "GNU release toolchain changed the explicit target identity")
endif()
foreach(_pair IN ITEMS
        "CMAKE_C_COMPILER|bin/gcc"
        "CMAKE_CXX_COMPILER|bin/g++"
        "CMAKE_ASM_COMPILER|bin/gcc"
        "CMAKE_AR|bin/ar"
        "CMAKE_RANLIB|bin/ranlib"
        "CMAKE_STRIP|bin/strip"
        "CMAKE_NM|bin/nm"
        "CMAKE_OBJCOPY|bin/objcopy"
        "AROS_AS_BIN|bin/as"
        "AROS_LINKER_BIN|bin/ld"
        "AROS_COLLECT_BIN|bin/collect-aros")
    string(REPLACE "|" ";" _parts "${_pair}")
    list(GET _parts 0 _variable)
    list(GET _parts 1 _relative)
    if(NOT "${${_variable}}" STREQUAL "${AROS_CROSS_TOOLCHAIN_ROOT}/${_relative}")
        message(FATAL_ERROR "GNU role ${_variable} was not selected from the prefix")
    endif()
endforeach()

foreach(_flag IN ITEMS -march=rva22u64 -mabi=lp64d -mcmodel=medany -mstrict-align)
    if(NOT _flag IN_LIST AROS_GNU_TARGET_COMPILE_OPTIONS)
        message(FATAL_ERROR "GNU target contract did not publish ${_flag}")
    endif()
    foreach(_language IN ITEMS C CXX ASM)
        if(NOT "${CMAKE_${_language}_FLAGS_INIT}" MATCHES "(^| )${_flag}($| )")
            message(FATAL_ERROR "GNU ${_language} initial flags omit ${_flag}")
        endif()
    endforeach()
endforeach()

set(_expected_builtins "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/gcc/libgcc.a")
if(NOT AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE STREQUAL _expected_builtins OR
   NOT AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES STREQUAL
       "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/gcc/libstdc++.a;${AROS_CROSS_TOOLCHAIN_ROOT}/lib/gcc/libsupc++.a;${_expected_builtins}")
    message(FATAL_ERROR "GNU compiler runtime queries did not resolve the prefix archives")
endif()

foreach(_platform_variable IN ITEMS AROS_TOOLCHAIN AROS_CROSS_TOOLCHAIN_ROOT
        AROS_TARGET_CPU AROS_TARGET_PLATFORM AROS_TARGET_PROFILE
        AROS_CROSS_TOOLCHAIN_PROFILE AROS_TARGET_TRIPLE
        AROS_AS_BIN AROS_LINKER_BIN AROS_COLLECT_BIN AROS_GNU_TARGET_COMPILE_OPTIONS
        AROS_GNU_TARGET_ARCHITECTURE AROS_GNU_TARGET_ATOMIC_ABI
        AROS_GNU_TARGET_X3_REG_USAGE AROS_CROSS_TOOLCHAIN_RELEASE_ID
        AROS_CROSS_TOOLCHAIN_TREE_SHA256 AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE
        AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES)
    if(NOT _platform_variable IN_LIST CMAKE_TRY_COMPILE_PLATFORM_VARIABLES)
        message(FATAL_ERROR "GNU toolchain does not propagate ${_platform_variable} into try_compile")
    endif()
endforeach()
