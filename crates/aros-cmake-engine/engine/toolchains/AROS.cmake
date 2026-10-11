# AROS-NX release-toolchain contract.
#
# This file is loaded by CMake before project(), so the selected compiler can
# never drift to a host installation after a build tree has been configured.
# `aros-cli` passes AROS_CROSS_TOOLCHAIN_ROOT explicitly. Direct CMake users
# may provide the same cache variable or environment variable.

# GNU toolchains have an inventory-bound executable layout and a RISC-V
# target contract. Keep that consumer separate from the legacy LLVM prefix
# contract below: selecting by family before the CPU mapping is what lets
# multiple GNU profiles share one architecture without inheriting an LLVM
# profile's board, triple, or runtime assumptions.
if(AROS_TOOLCHAIN STREQUAL "gnu")
    include("${CMAKE_CURRENT_LIST_DIR}/GnuAROS.cmake")
    return()
endif()

if(NOT AROS_CROSS_TOOLCHAIN_ROOT AND
   DEFINED ENV{AROS_CROSS_TOOLCHAIN_ROOT} AND
   NOT "$ENV{AROS_CROSS_TOOLCHAIN_ROOT}" STREQUAL "")
    set(AROS_CROSS_TOOLCHAIN_ROOT "$ENV{AROS_CROSS_TOOLCHAIN_ROOT}")
endif()
if(NOT AROS_CROSS_TOOLCHAIN_ROOT)
    message(FATAL_ERROR
        "AROS release toolchain requires AROS_CROSS_TOOLCHAIN_ROOT. "
        "Install one with `aros toolchain install --preset <name>` or pass "
        "an explicitly built upstream CROSSTOOLSDIR.")
endif()

cmake_path(ABSOLUTE_PATH AROS_CROSS_TOOLCHAIN_ROOT
    NORMALIZE OUTPUT_VARIABLE _aros_cross_root)
if(NOT IS_DIRECTORY "${_aros_cross_root}")
    message(FATAL_ERROR
        "AROS cross-toolchain root does not exist: ${_aros_cross_root}")
endif()
set(AROS_CROSS_TOOLCHAIN_ROOT "${_aros_cross_root}" CACHE PATH
    "Immutable AROS cross-toolchain prefix" FORCE)

if(NOT AROS_TARGET_CPU)
    if(CMAKE_SYSTEM_PROCESSOR)
        set(AROS_TARGET_CPU "${CMAKE_SYSTEM_PROCESSOR}")
    else()
        message(FATAL_ERROR
            "AROS release toolchain requires AROS_TARGET_CPU")
    endif()
endif()

if(AROS_TARGET_CPU STREQUAL "x86_64")
    set(_aros_expected_platform "pc")
    set(_aros_profile "pc-x86_64")
    set(_aros_triple "x86_64-unknown-aros")
    set(_aros_builtins "x86_64")
elseif(AROS_TARGET_CPU STREQUAL "arm")
    set(_aros_expected_platform "raspi")
    set(_aros_profile "arm-raspi")
    set(_aros_triple "arm-unknown-aros")
    set(_aros_builtins "armhf")
elseif(AROS_TARGET_CPU STREQUAL "aarch64")
    set(_aros_expected_platform "raspi")
    set(_aros_profile "rpi-aarch64")
    set(_aros_triple "aarch64-unknown-aros")
    set(_aros_builtins "aarch64")
elseif(AROS_TARGET_CPU STREQUAL "riscv64")
    set(_aros_expected_platform "opensbi")
    set(_aros_profile "opensbi-riscv64")
    set(_aros_triple "riscv64-unknown-aros")
    set(_aros_builtins "riscv64")
else()
    message(FATAL_ERROR
        "No release-toolchain contract exists for CPU '${AROS_TARGET_CPU}'")
endif()
if(AROS_TARGET_PLATFORM AND
   NOT AROS_TARGET_PLATFORM STREQUAL _aros_expected_platform)
    message(FATAL_ERROR
        "CPU ${AROS_TARGET_CPU} requires platform ${_aros_expected_platform}, "
        "not ${AROS_TARGET_PLATFORM}")
endif()
set(AROS_TARGET_PLATFORM "${_aros_expected_platform}" CACHE STRING
    "AROS machine platform" FORCE)
set(AROS_TARGET_PROFILE "${_aros_profile}" CACHE STRING
    "Locked AROS target profile" FORCE)
set(AROS_TARGET_TRIPLE "${_aros_triple}" CACHE STRING
    "AROS compiler target triple" FORCE)

function(_aros_manifest_require_type json label expected_type)
    string(JSON _actual_type ERROR_VARIABLE _json_error TYPE "${json}" ${ARGN})
    if(NOT _json_error STREQUAL "NOTFOUND" OR
       NOT _actual_type STREQUAL expected_type)
        message(FATAL_ERROR "AROS toolchain ${label} must be ${expected_type}")
    endif()
endfunction()

function(_aros_manifest_require_members json label expected_fields)
    string(JSON _length ERROR_VARIABLE _json_error LENGTH "${json}" ${ARGN})
    if(NOT _json_error STREQUAL "NOTFOUND")
        message(FATAL_ERROR "AROS toolchain ${label} is not a JSON object")
    endif()
    set(_expected ${expected_fields})
    list(LENGTH _expected _expected_length)
    if(NOT _length EQUAL _expected_length)
        message(FATAL_ERROR
            "AROS toolchain ${label} has ${_length} fields; expected ${_expected_length}")
    endif()
    set(_seen "")
    math(EXPR _last "${_length} - 1")
    foreach(_index RANGE 0 ${_last})
        string(JSON _member MEMBER "${json}" ${ARGN} ${_index})
        if(NOT _member IN_LIST _expected OR _member IN_LIST _seen)
            message(FATAL_ERROR "AROS toolchain ${label} has invalid field '${_member}'")
        endif()
        list(APPEND _seen "${_member}")
    endforeach()
    foreach(_member IN LISTS _expected)
        if(NOT _member IN_LIST _seen)
            message(FATAL_ERROR "AROS toolchain ${label} omits field '${_member}'")
        endif()
    endforeach()
endfunction()

function(_aros_manifest_require_absent json label member)
    string(JSON _member_type ERROR_VARIABLE _json_error TYPE "${json}" "${member}")
    if(_json_error STREQUAL "NOTFOUND")
        message(FATAL_ERROR "AROS toolchain ${label} must not contain ${member}")
    endif()
endfunction()

function(_aros_manifest_validate_llvm_version value schema)
    if(NOT "${value}" MATCHES "^[0-9]+\\.[0-9]+\\.[0-9]+$")
        message(FATAL_ERROR "AROS toolchain LLVM version must have three numeric components")
    endif()
    # Schema 1 preserves its historical unbounded numeric grammar. Only the
    # family-v2 compiler descriptor adopts the common bounded version contract.
    if(schema EQUAL 1)
        return()
    endif()
    string(REPLACE "." ";" _components "${value}")
    foreach(_component IN LISTS _components)
        string(LENGTH "${_component}" _component_length)
        if(_component_length GREATER 10)
            message(FATAL_ERROR "AROS toolchain LLVM version has an invalid component")
        endif()
        string(REGEX REPLACE "^0+" "" _numeric_component "${_component}")
        if(_numeric_component STREQUAL "")
            set(_numeric_component 0)
        endif()
        if(_numeric_component GREATER 2147483647)
            message(FATAL_ERROR "AROS toolchain LLVM version has an invalid component")
        endif()
    endforeach()
endfunction()

# CMake loads this file again inside compiler checks. Custom cache variables
# are not copied into those try_compile projects unless the toolchain declares
# them as platform inputs, so preserve the fail-closed root/profile contract
# across that boundary as well.
list(APPEND CMAKE_TRY_COMPILE_PLATFORM_VARIABLES
    AROS_CROSS_TOOLCHAIN_ROOT
    AROS_TARGET_CPU
    AROS_TARGET_PLATFORM
    AROS_NATIVE_CONSUMER_VALIDATED_GENERIC_SDK
    GCC_CONFIG_FLOAT_ABI)
list(REMOVE_DUPLICATES CMAKE_TRY_COMPILE_PLATFORM_VARIABLES)

# Release assets carry a manifest. Locally built legacy prefixes remain
# usable, but when a manifest exists it is authoritative and mismatches fail
# before any compiler or runtime probing.
set(_aros_manifest "${_aros_cross_root}/toolchain-manifest.json")
unset(_aros_manifest_llvm_version)
if(EXISTS "${_aros_manifest}")
    file(READ "${_aros_manifest}" _aros_manifest_json)
    _aros_manifest_require_type("${_aros_manifest_json}" "manifest schema" NUMBER schema)
    string(JSON _aros_manifest_schema GET "${_aros_manifest_json}" schema)
    if(NOT _aros_manifest_schema MATCHES "^[12]$")
        message(FATAL_ERROR
            "Unsupported AROS toolchain manifest schema ${_aros_manifest_schema}")
    endif()
    foreach(_aros_field IN ITEMS
            release_id host target_profile target_triple tree_sha256)
        _aros_manifest_require_type(
            "${_aros_manifest_json}" "manifest ${_aros_field}" STRING "${_aros_field}")
        string(JSON _aros_manifest_${_aros_field}
            ERROR_VARIABLE _aros_json_error
            GET "${_aros_manifest_json}" "${_aros_field}")
        if(NOT _aros_json_error STREQUAL "NOTFOUND")
            message(FATAL_ERROR
                "Invalid ${_aros_manifest}: missing ${_aros_field}: ${_aros_json_error}")
        endif()
    endforeach()

    if(_aros_manifest_schema EQUAL 1)
        _aros_manifest_require_absent("${_aros_manifest_json}" "schema 1 manifest" compiler)
        _aros_manifest_require_type(
            "${_aros_manifest_json}" "manifest llvm_version" STRING llvm_version)
        string(JSON _aros_manifest_llvm_version GET "${_aros_manifest_json}" llvm_version)
    else()
        _aros_manifest_require_absent(
            "${_aros_manifest_json}" "schema 2 manifest" llvm_version)
        _aros_manifest_require_type(
            "${_aros_manifest_json}" "manifest compiler" OBJECT compiler)
        string(JSON _aros_manifest_compiler GET "${_aros_manifest_json}" compiler)
        _aros_manifest_require_type(
            "${_aros_manifest_compiler}" "manifest compiler family" STRING family)
        string(JSON _aros_manifest_compiler_family GET "${_aros_manifest_compiler}" family)
        if(NOT _aros_manifest_compiler_family STREQUAL "llvm")
            message(FATAL_ERROR "AROS toolchain manifest has a non-LLVM compiler identity")
        endif()
        _aros_manifest_require_members(
            "${_aros_manifest_compiler}" "manifest compiler" "family;version")
        _aros_manifest_require_type(
            "${_aros_manifest_compiler}" "manifest compiler version" STRING version)
        string(JSON _aros_manifest_llvm_version GET "${_aros_manifest_compiler}" version)
    endif()
    _aros_manifest_validate_llvm_version("${_aros_manifest_llvm_version}" "${_aros_manifest_schema}")

    foreach(_aros_field IN ITEMS release_id host target_profile target_triple)
        if("${_aros_manifest_${_aros_field}}" STREQUAL "")
            message(FATAL_ERROR
                "AROS toolchain manifest has an empty ${_aros_field}")
        endif()
    endforeach()
    if(NOT _aros_manifest_target_profile STREQUAL _aros_profile OR
       NOT _aros_manifest_target_triple STREQUAL _aros_triple)
        message(FATAL_ERROR
            "AROS toolchain manifest selects ${_aros_manifest_target_profile}/"
            "${_aros_manifest_target_triple}, expected ${_aros_profile}/${_aros_triple}")
    endif()
    string(LENGTH "${_aros_manifest_tree_sha256}" _aros_tree_digest_length)
    if(NOT _aros_tree_digest_length EQUAL 64 OR
       NOT _aros_manifest_tree_sha256 MATCHES "^[0-9a-fA-F]+$")
        message(FATAL_ERROR
            "AROS toolchain manifest has an invalid tree_sha256")
    endif()
    set(AROS_CROSS_TOOLCHAIN_RELEASE_ID "${_aros_manifest_release_id}"
        CACHE INTERNAL "AROS release-toolchain identity" FORCE)
    set(AROS_CROSS_TOOLCHAIN_TREE_SHA256 "${_aros_manifest_tree_sha256}"
        CACHE INTERNAL "AROS release-toolchain tree digest" FORCE)
else()
    set(AROS_CROSS_TOOLCHAIN_RELEASE_ID "local:${_aros_cross_root}"
        CACHE INTERNAL "Local AROS toolchain identity" FORCE)
    set(AROS_CROSS_TOOLCHAIN_TREE_SHA256 ""
        CACHE INTERNAL "Local AROS toolchain tree digest" FORCE)
endif()

set(_aros_required_tools
    clang clang++ ld.lld llvm-ar llvm-ranlib llvm-nm llvm-strip
    llvm-objcopy llvm-objdump)
foreach(_aros_tool IN LISTS _aros_required_tools)
    if(NOT EXISTS "${_aros_cross_root}/bin/${_aros_tool}" OR
       IS_DIRECTORY "${_aros_cross_root}/bin/${_aros_tool}")
        message(FATAL_ERROR
            "AROS toolchain ${_aros_profile} lacks bin/${_aros_tool}")
    endif()
endforeach()

set(_aros_required_runtime
    "include/c++/v1/algorithm"
    "include/c++/v1/cerrno"
    "include/c++/v1/cinttypes"
    "include/c++/v1/cstddef"
    "include/c++/v1/cstdint"
    "include/c++/v1/deque"
    "include/c++/v1/memory"
    "include/c++/v1/string"
    "include/c++/v1/system_error"
    "include/c++/v1/vector"
    "lib/libc++.a"
    "lib/libc++abi.a"
    "lib/libunwind.a")
foreach(_aros_path IN LISTS _aros_required_runtime)
    if(NOT EXISTS "${_aros_cross_root}/${_aros_path}" OR
       IS_DIRECTORY "${_aros_cross_root}/${_aros_path}")
        message(FATAL_ERROR
            "AROS toolchain ${_aros_profile} lacks ${_aros_path}")
    endif()
endforeach()
file(GLOB _aros_builtins_archives
    "${_aros_cross_root}/lib/clang/*/lib/aros/libclang_rt.builtins-${_aros_builtins}.a")
if(NOT _aros_builtins_archives)
    message(FATAL_ERROR
        "AROS toolchain ${_aros_profile} lacks compiler-rt builtins for ${_aros_builtins}")
endif()
list(SORT _aros_builtins_archives)
list(LENGTH _aros_builtins_archives _aros_builtins_archive_count)
if(NOT _aros_builtins_archive_count EQUAL 1)
    message(FATAL_ERROR
        "AROS toolchain ${_aros_profile} has an ambiguous compiler-rt builtins "
        "selection for ${_aros_builtins}: ${_aros_builtins_archives}")
endif()
list(GET _aros_builtins_archives 0 _aros_builtins_archive)
if(DEFINED _aros_manifest_llvm_version)
    set(_aros_expected_builtins_archive
        "${_aros_cross_root}/lib/clang/${_aros_manifest_llvm_version}/lib/aros/libclang_rt.builtins-${_aros_builtins}.a")
    if(NOT _aros_builtins_archive STREQUAL _aros_expected_builtins_archive)
        message(FATAL_ERROR
            "AROS toolchain ${_aros_profile} compiler-rt builtins do not match LLVM version "
            "${_aros_manifest_llvm_version}")
    endif()
endif()
if(AROS_TARGET_CPU STREQUAL "x86_64")
    file(GLOB _aros_i386_builtins
        "${_aros_cross_root}/lib/clang/*/lib/aros/libclang_rt.builtins-i386.a")
    if(NOT _aros_i386_builtins)
        message(FATAL_ERROR
            "AROS x86_64 release toolchain lacks its i386 compiler-rt companion")
    endif()
    list(SORT _aros_i386_builtins)
    list(LENGTH _aros_i386_builtins _aros_i386_builtins_count)
    if(NOT _aros_i386_builtins_count EQUAL 1)
        message(FATAL_ERROR
            "AROS x86_64 release toolchain has an ambiguous i386 compiler-rt "
            "companion: ${_aros_i386_builtins}")
    endif()
    list(GET _aros_i386_builtins 0 _aros_i386_builtins_archive)
    if(DEFINED _aros_manifest_llvm_version)
        set(_aros_expected_i386_builtins_archive
            "${_aros_cross_root}/lib/clang/${_aros_manifest_llvm_version}/lib/aros/libclang_rt.builtins-i386.a")
        if(NOT _aros_i386_builtins_archive STREQUAL _aros_expected_i386_builtins_archive)
            message(FATAL_ERROR
                "AROS x86_64 i386 compiler-rt companion does not match LLVM version "
                "${_aros_manifest_llvm_version}")
        endif()
    endif()
    set(AROS_CROSS_TOOLCHAIN_COMPANION_TRIPLE "i386-unknown-aros"
        CACHE INTERNAL "Validated release-toolchain companion triple" FORCE)
    set(AROS_CROSS_TOOLCHAIN_COMPANION_BUILTINS_ARCHIVE
        "${_aros_i386_builtins_archive}" CACHE FILEPATH
        "Validated release-toolchain companion compiler-rt archive" FORCE)
else()
    set(AROS_CROSS_TOOLCHAIN_COMPANION_TRIPLE "" CACHE INTERNAL
        "Validated release-toolchain companion triple" FORCE)
    set(AROS_CROSS_TOOLCHAIN_COMPANION_BUILTINS_ARCHIVE "" CACHE INTERNAL
        "Validated release-toolchain companion compiler-rt archive" FORCE)
endif()

# A release prefix intentionally ships only the C++ runtime needed by an
# external CMake consumer.  Its locked direct-ld.lld partial links use this
# exact, prefix-owned archive set.  Keep absolute archive names here, after
# validating them once, so the consumer cannot resolve a host library through
# a search path by accident.
set(AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE "${_aros_builtins_archive}"
    CACHE FILEPATH "Prefix-owned compiler-rt archive for locked C++ links" FORCE)
set(AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES
    "${_aros_cross_root}/lib/libc++.a"
    "${_aros_cross_root}/lib/libc++abi.a"
    "${_aros_cross_root}/lib/libunwind.a"
    "${_aros_builtins_archive}"
    CACHE INTERNAL
    "Prefix-owned C++ runtime archives for locked AROS partial links" FORCE)

set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_SYSTEM_PROCESSOR "${AROS_TARGET_CPU}")
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)

set(CMAKE_C_COMPILER "${_aros_cross_root}/bin/clang" CACHE FILEPATH "" FORCE)
set(CMAKE_CXX_COMPILER "${_aros_cross_root}/bin/clang++" CACHE FILEPATH "" FORCE)
set(CMAKE_ASM_COMPILER "${_aros_cross_root}/bin/clang" CACHE FILEPATH "" FORCE)
set(CMAKE_C_COMPILER_TARGET "${_aros_triple}")
set(CMAKE_CXX_COMPILER_TARGET "${_aros_triple}")
set(CMAKE_ASM_COMPILER_TARGET "${_aros_triple}")
set(CMAKE_AR "${_aros_cross_root}/bin/llvm-ar" CACHE FILEPATH "" FORCE)
set(CMAKE_RANLIB "${_aros_cross_root}/bin/llvm-ranlib" CACHE FILEPATH "" FORCE)
set(CMAKE_NM "${_aros_cross_root}/bin/llvm-nm" CACHE FILEPATH "" FORCE)
set(CMAKE_STRIP "${_aros_cross_root}/bin/llvm-strip" CACHE FILEPATH "" FORCE)
set(CMAKE_OBJCOPY "${_aros_cross_root}/bin/llvm-objcopy" CACHE FILEPATH "" FORCE)
set(CMAKE_OBJDUMP "${_aros_cross_root}/bin/llvm-objdump" CACHE FILEPATH "" FORCE)
set(AROS_LLD_BIN "${_aros_cross_root}/bin/ld.lld" CACHE FILEPATH
    "AROS relocatable module linker" FORCE)

if(AROS_TARGET_CPU STREQUAL "arm")
    if(AROS_NATIVE_CONSUMER_VALIDATED_GENERIC_SDK)
        # The language-rule hook reconciles ENV and INIT with the sealed
        # source ABI before cache initialization. Appending here would
        # duplicate a matching environment flag (or this file's second load).
        # Do not infer CPU/FPU choices from the board selector either.
    else()
        # Preserve the established release profile for legacy consumers.
        string(APPEND CMAKE_C_FLAGS_INIT " -mcpu=cortex-a7 -mfpu=neon-vfpv4 -mfloat-abi=hard")
        string(APPEND CMAKE_CXX_FLAGS_INIT " -mcpu=cortex-a7 -mfpu=neon-vfpv4 -mfloat-abi=hard")
        string(APPEND CMAKE_ASM_FLAGS_INIT " -mcpu=cortex-a7 -mfpu=neon-vfpv4 -mfloat-abi=hard")
    endif()
endif()
