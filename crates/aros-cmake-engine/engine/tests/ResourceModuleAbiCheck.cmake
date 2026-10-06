# Not collected by the gate loop (it only runs *Test.cmake files). This check
# reads arch/riscv-esp32p4/flashdisk from the AROS tree, which the source
# contract's integration commit does not carry yet. Rename it to
# ResourceModuleAbiTest.cmake in the change that moves the pin to a tree with
# the port; until then run it by hand against such a tree.
cmake_minimum_required(VERSION 3.24)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

find_program(_ninja NAMES ninja ninja-build)
if(NOT _ninja)
    message(FATAL_ERROR "ResourceModuleAbiTest requires Ninja")
endif()
find_program(_host_cc NAMES cc)
if(NOT _host_cc)
    message(FATAL_ERROR "ResourceModuleAbiTest requires a host C compiler")
endif()

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    file(REAL_PATH "$ENV{TMPDIR}" _temporary_parent)
else()
    file(REAL_PATH "/tmp" _temporary_parent)
endif()
string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temporary_parent}/aros-resource-module-abi-${_suffix}")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

set(_fixture "${CMAKE_CURRENT_LIST_DIR}/resource-module-abi")
set(_reference_project "${CMAKE_CURRENT_LIST_DIR}/reference-genmodule")
set(_reference_source "${AROS_TEST_TREE}/tools/genmodule")
set(_reference_build "${_root}/reference-build")
set(_host_genmodule "${_reference_build}/genmodule")
set(_build "${_root}/build")
set(_kernel_dir "${AROS_TEST_TREE}/rom/kernel")
set(_kernel_config "${_kernel_dir}/kernel.conf")
set(_filesystem_config "${AROS_TEST_TREE}/rom/filesystem/FileSystem.conf")
set(_card_config "${AROS_TEST_TREE}/rom/card/card.conf")
set(_flashdisk_dir "${AROS_TEST_TREE}/arch/riscv-esp32p4/flashdisk")
set(_flashdisk_config "${_flashdisk_dir}/flashdisk.conf")
set(_synthetic_base_config "${_root}/synthetic-device-base.conf")
set(_synthetic_override_config "${_root}/synthetic-device-override.conf")

# Keep the synthetic device small and private to this fixture. The base config
# has AUTO includes and only the standard device pair; the later override adds
# cdef, an extra function, and an include-name override.
file(WRITE "${_synthetic_base_config}" [=[
##begin config
basename SyntheticDevice
version 1.0
libbase SyntheticDeviceBase
libbasetype struct SyntheticDeviceBase
beginio_func BeginIO
abortio_func AbortIO
##end config
]=])
file(WRITE "${_synthetic_override_config}" [=[
##begin config
includename override_device
##end config
##begin cdef
struct SyntheticDeviceBase
{
    unsigned long marker;
};
##end cdef
##begin functionlist
ULONG SyntheticDeviceExtra(ULONG value) (D0)
##end functionlist
]=])

# Build a fresh reference host tool from the exact source tree used by the
# engine tests. No cached or Make-built genmodule binary is consulted.
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_reference_project}"
        -B "${_reference_build}" -G Ninja
        "-DAROS_GENMODULE_SOURCE_DIR=${_reference_source}"
        "-DCMAKE_C_COMPILER=${_host_cc}"
    RESULT_VARIABLE _reference_configure_result
    OUTPUT_VARIABLE _reference_configure_stdout
    ERROR_VARIABLE _reference_configure_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _reference_configure_result EQUAL 0)
    message(FATAL_ERROR
        "reference genmodule configure failed (${_reference_configure_result})\n"
        "${_reference_configure_stdout}\n${_reference_configure_stderr}")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_reference_build}"
        --target genmodule --parallel 2
    RESULT_VARIABLE _reference_build_result
    OUTPUT_VARIABLE _reference_build_stdout
    ERROR_VARIABLE _reference_build_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _reference_build_result EQUAL 0 OR
   NOT EXISTS "${_host_genmodule}" OR IS_DIRECTORY "${_host_genmodule}")
    message(FATAL_ERROR
        "reference genmodule build failed (${_reference_build_result})\n"
        "${_reference_build_stdout}\n${_reference_build_stderr}")
endif()

# Independently record the exact reference outputs from the three operations
# exercised by the full ABI endpoint.
set(_reference_root "${_root}/direct-reference")
set(_reference_include "${_reference_root}/include")
set(_reference_gen "${_reference_root}/gen")
set(_reference_fd "${_reference_root}/fd")
set(_filesystem_reference_root "${_root}/filesystem-reference")
set(_filesystem_reference_gen "${_filesystem_reference_root}/gen")
set(_filesystem_reference_fd "${_filesystem_reference_root}/fd")
set(_card_reference_root "${_root}/card-reference")
set(_card_reference_include "${_card_reference_root}/include")
set(_card_reference_gen "${_card_reference_root}/gen")
set(_card_reference_fd "${_card_reference_root}/fd")
set(_flashdisk_reference_root "${_root}/flashdisk-reference")
set(_flashdisk_reference_include "${_flashdisk_reference_root}/include")
set(_flashdisk_reference_gen "${_flashdisk_reference_root}/gen")
set(_flashdisk_reference_fd "${_flashdisk_reference_root}/fd")
set(_synthetic_reference_root "${_root}/synthetic-device-reference")
set(_synthetic_reference_include "${_synthetic_reference_root}/include")
set(_synthetic_reference_gen "${_synthetic_reference_root}/gen")
set(_synthetic_reference_fd "${_synthetic_reference_root}/fd")
file(MAKE_DIRECTORY
    "${_reference_include}/clib"
    "${_reference_include}/inline"
    "${_reference_include}/defines"
    "${_reference_include}/proto"
    "${_reference_gen}"
    "${_reference_fd}"
    "${_filesystem_reference_gen}"
    "${_filesystem_reference_fd}"
    "${_card_reference_include}/clib"
    "${_card_reference_include}/inline"
    "${_card_reference_include}/defines"
    "${_card_reference_include}/proto"
    "${_card_reference_gen}"
    "${_card_reference_fd}"
    "${_flashdisk_reference_include}/clib"
    "${_flashdisk_reference_include}/inline"
    "${_flashdisk_reference_include}/defines"
    "${_flashdisk_reference_include}/proto"
    "${_flashdisk_reference_gen}"
    "${_flashdisk_reference_fd}"
    "${_synthetic_reference_include}/clib"
    "${_synthetic_reference_include}/inline"
    "${_synthetic_reference_include}/defines"
    "${_synthetic_reference_include}/proto"
    "${_synthetic_reference_gen}"
    "${_synthetic_reference_fd}")

function(_run_reference_genmodule operation config module modtype directory)
    execute_process(
        COMMAND "${_host_genmodule}" -c "${config}" -d "${directory}"
            "${operation}" "${module}" "${modtype}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "direct reference genmodule ${operation} failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    endif()
endfunction()

function(_run_reference_genmodule_with_override
        operation config override module modtype directory)
    execute_process(
        COMMAND "${_host_genmodule}" -c "${config}" -o "${override}"
            -d "${directory}" "${operation}" "${module}" "${modtype}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "direct reference genmodule ${operation} with override failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    endif()
endfunction()

_run_reference_genmodule(writeincludes "${_kernel_config}" kernel resource
    "${_reference_include}")
_run_reference_genmodule(writelibdefs "${_kernel_config}" kernel resource
    "${_reference_gen}")
_run_reference_genmodule(writefd "${_kernel_config}" kernel resource
    "${_reference_fd}")

# FileSystem is full ABI despite `noincludes` and an empty functionlist. Its
# concrete support is libdefs, with neither an FD nor a public header set.
_run_reference_genmodule(writelibdefs "${_filesystem_config}" FileSystem resource
    "${_filesystem_reference_gen}")
_run_reference_genmodule(writefd "${_filesystem_config}" FileSystem resource
    "${_filesystem_reference_fd}")
if(EXISTS "${_filesystem_reference_fd}/FileSystem_lib.fd")
    message(FATAL_ERROR
        "reference genmodule emitted FileSystem FD despite noincludes/empty functionlist")
endif()

# The card config-derived include name is cardres, but libdefs stays keyed by
# module name card. Keep the independent reference outputs separate.
_run_reference_genmodule(writeincludes "${_card_config}" card resource
    "${_card_reference_include}")
_run_reference_genmodule(writelibdefs "${_card_config}" card resource
    "${_card_reference_gen}")
_run_reference_genmodule(writefd "${_card_config}" card resource
    "${_card_reference_fd}")

# The P4 flashdisk config has no explicit functionlist: genmodule's validated
# BeginIO/AbortIO pair still produces the full device ABI.
_run_reference_genmodule(writeincludes "${_flashdisk_config}" flashdisk device
    "${_flashdisk_reference_include}")
_run_reference_genmodule(writelibdefs "${_flashdisk_config}" flashdisk device
    "${_flashdisk_reference_gen}")
_run_reference_genmodule(writefd "${_flashdisk_config}" flashdisk device
    "${_flashdisk_reference_fd}")

# Direct reference evaluation must preserve the base noincludes AUTO choice,
# even though the later override adds cdef and functionlist facts.
_run_reference_genmodule_with_override(writeincludes
    "${_synthetic_base_config}" "${_synthetic_override_config}"
    SyntheticDevice device "${_synthetic_reference_include}")
_run_reference_genmodule_with_override(writelibdefs
    "${_synthetic_base_config}" "${_synthetic_override_config}"
    SyntheticDevice device "${_synthetic_reference_gen}")
_run_reference_genmodule_with_override(writefd
    "${_synthetic_base_config}" "${_synthetic_override_config}"
    SyntheticDevice device "${_synthetic_reference_fd}")
if(EXISTS "${_synthetic_reference_fd}/override_device_lib.fd")
    message(FATAL_ERROR
        "reference genmodule lost base noincludes choice across device override")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        "-DAROS_HOST_GENMODULE=${_host_genmodule}"
        "-DAROS_TEST_DEVICE_BASE_CONFIG=${_synthetic_base_config}"
        "-DAROS_TEST_DEVICE_OVERRIDE_CONFIG=${_synthetic_override_config}"
        ${AROS_TEST_TOOL_ARGS}
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "resource module ABI fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target probe-kernel-includes
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "probe-kernel-includes build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

function(_compare_reference expected actual)
    if(NOT EXISTS "${expected}" OR IS_DIRECTORY "${expected}" OR
       NOT EXISTS "${actual}" OR IS_DIRECTORY "${actual}")
        message(FATAL_ERROR
            "missing generated/reference ABI file: expected=${expected}, actual=${actual}")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E compare_files "${expected}" "${actual}"
        RESULT_VARIABLE _compare_result
        OUTPUT_VARIABLE _compare_stdout
        ERROR_VARIABLE _compare_stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    if(NOT _compare_result EQUAL 0)
        message(FATAL_ERROR
            "ABI output differs from direct genmodule reference:\n"
            "  reference: ${expected}\n  generated: ${actual}\n"
            "${_compare_stdout}\n${_compare_stderr}")
    endif()
endfunction()

string(MAKE_C_IDENTIFIER "probe-kernel" _safe_id)
set(_private_root "${_build}/genmodule/rom/kernel/${_safe_id}")
set(_private_include "${_private_root}/include")
set(_headers
    "clib/kernel_protos.h"
    "inline/kernel.h"
    "defines/kernel.h"
    "defines/kernel_LVO.h"
    "proto/kernel.h")
set(_public_roots
    "${_build}/SDK/include"
    "${_build}/GENINCDIR"
    "${_build}/SYS/Developer/include")

function(_compare_all_abi_outputs)
    foreach(_header IN LISTS _headers)
        _compare_reference("${_reference_include}/${_header}"
            "${_private_include}/${_header}")
        foreach(_root IN LISTS _public_roots)
            _compare_reference("${_reference_include}/${_header}"
                "${_root}/${_header}")
        endforeach()
    endforeach()
    _compare_reference("${_reference_gen}/kernel_libdefs.h"
        "${_private_root}/gen/kernel_libdefs.h")
    _compare_reference("${_reference_fd}/kernel_lib.fd"
        "${_private_root}/fd/kernel_lib.fd")
    _compare_reference("${_reference_fd}/kernel_lib.fd"
        "${_build}/SYS/Developer/SDK/fd/kernel_lib.fd")
endfunction()

_compare_all_abi_outputs()

# FileSystem's noincludes full ABI still has a real includes endpoint because
# that source target depends on ABI support. It materialises libdefs but the
# reference genmodule emits neither an FD nor public headers for this config.
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target kernel-filesystem-includes
    RESULT_VARIABLE _filesystem_build_result
    OUTPUT_VARIABLE _filesystem_build_stdout
    ERROR_VARIABLE _filesystem_build_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _filesystem_build_result EQUAL 0)
    message(FATAL_ERROR
        "kernel-filesystem-includes build failed (${_filesystem_build_result})\n"
        "${_filesystem_build_stdout}\n${_filesystem_build_stderr}")
endif()

string(MAKE_C_IDENTIFIER "kernel-filesystem" _filesystem_safe_id)
set(_filesystem_private_root
    "${_build}/genmodule/rom/filesystem/${_filesystem_safe_id}")
function(_compare_filesystem_outputs)
    _compare_reference("${_filesystem_reference_gen}/FileSystem_libdefs.h"
        "${_filesystem_private_root}/gen/FileSystem_libdefs.h")
endfunction()

function(_assert_no_filesystem_headers)
    set(_header_roots "${_filesystem_private_root}/include" ${_public_roots})
    set(_filesystem_headers
        "clib/FileSystem_protos.h"
        "inline/FileSystem.h"
        "defines/FileSystem.h"
        "defines/FileSystem_LVO.h"
        "proto/FileSystem.h")
    foreach(_root IN LISTS _header_roots)
        foreach(_header IN LISTS _filesystem_headers)
            if(EXISTS "${_root}/${_header}")
                message(FATAL_ERROR
                    "noincludes FileSystem unexpectedly produced ${_root}/${_header}")
            endif()
        endforeach()
    endforeach()
    foreach(_fd IN ITEMS
            "${_filesystem_private_root}/fd/FileSystem_lib.fd"
            "${_build}/SYS/Developer/SDK/fd/FileSystem_lib.fd")
        if(EXISTS "${_fd}")
            message(FATAL_ERROR
                "noincludes FileSystem unexpectedly produced FD ${_fd}")
        endif()
    endforeach()
    file(READ "${_build}/build.ninja" _ninja_graph)
    if(_ninja_graph MATCHES "FileSystem(_protos|_LVO)?[.]h")
        message(FATAL_ERROR
            "noincludes FileSystem declared a generated public header in Ninja")
    endif()
    if(_ninja_graph MATCHES "FileSystem_lib[.]fd")
        message(FATAL_ERROR "noincludes FileSystem declared an FD output in Ninja")
    endif()
endfunction()

_compare_filesystem_outputs()
_assert_no_filesystem_headers()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target kernel-filesystem-includes
    RESULT_VARIABLE _filesystem_noop_result
    OUTPUT_VARIABLE _filesystem_noop_stdout
    ERROR_VARIABLE _filesystem_noop_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _filesystem_noop_result EQUAL 0 OR
   NOT _filesystem_noop_stdout MATCHES "no work to do")
    message(FATAL_ERROR
        "FileSystem ABI generation was not a no-op (${_filesystem_noop_result})\n"
        "${_filesystem_noop_stdout}\n${_filesystem_noop_stderr}")
endif()

file(REMOVE "${_filesystem_private_root}/gen/FileSystem_libdefs.h")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target kernel-filesystem-includes
    RESULT_VARIABLE _filesystem_repair_result
    OUTPUT_VARIABLE _filesystem_repair_stdout
    ERROR_VARIABLE _filesystem_repair_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _filesystem_repair_result EQUAL 0)
    message(FATAL_ERROR
        "FileSystem libdefs output repair failed (${_filesystem_repair_result})\n"
        "${_filesystem_repair_stdout}\n${_filesystem_repair_stderr}")
endif()
_compare_filesystem_outputs()
_assert_no_filesystem_headers()

# The card resource proves config-derived naming: module `card` publishes the
# five `cardres` headers and FD, while its libdefs filename remains `card`.
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target kernel-cardres-includes
    RESULT_VARIABLE _card_build_result
    OUTPUT_VARIABLE _card_build_stdout
    ERROR_VARIABLE _card_build_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _card_build_result EQUAL 0)
    message(FATAL_ERROR
        "kernel-cardres-includes build failed (${_card_build_result})\n"
        "${_card_build_stdout}\n${_card_build_stderr}")
endif()

string(MAKE_C_IDENTIFIER "kernel-cardres" _card_safe_id)
set(_card_private_root "${_build}/genmodule/rom/card/${_card_safe_id}")
set(_card_private_include "${_card_private_root}/include")
set(_card_headers
    "clib/cardres_protos.h"
    "inline/cardres.h"
    "defines/cardres.h"
    "defines/cardres_LVO.h"
    "proto/cardres.h")
foreach(_header IN LISTS _card_headers)
    _compare_reference("${_card_reference_include}/${_header}"
        "${_card_private_include}/${_header}")
    foreach(_root IN LISTS _public_roots)
        _compare_reference("${_card_reference_include}/${_header}"
            "${_root}/${_header}")
    endforeach()
endforeach()
_compare_reference("${_card_reference_gen}/card_libdefs.h"
    "${_card_private_root}/gen/card_libdefs.h")
_compare_reference("${_card_reference_fd}/cardres_lib.fd"
    "${_card_private_root}/fd/cardres_lib.fd")
_compare_reference("${_card_reference_fd}/cardres_lib.fd"
    "${_build}/SYS/Developer/SDK/fd/cardres_lib.fd")

set(_card_default_names
    "clib/card_protos.h"
    "inline/card.h"
    "defines/card.h"
    "defines/card_LVO.h"
    "proto/card.h")
set(_card_output_roots "${_card_private_include}" ${_public_roots})
foreach(_root IN LISTS _card_output_roots)
    foreach(_name IN LISTS _card_default_names)
        if(EXISTS "${_root}/${_name}")
            message(FATAL_ERROR
                "card config ignored includename cardres and produced ${_root}/${_name}")
        endif()
    endforeach()
endforeach()
foreach(_root IN ITEMS "${_card_private_root}/fd" "${_build}/SYS/Developer/SDK/fd")
    if(EXISTS "${_root}/card_lib.fd")
        message(FATAL_ERROR
            "card config ignored includename cardres and produced ${_root}/card_lib.fd")
    endif()
endforeach()

# The real flashdisk device is a source-owned full ABI even though its config
# supplies BeginIO/AbortIO instead of a functionlist. Compare all products to
# a fresh genmodule built from the same source tree.
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target kernel-flashdisk-includes
    RESULT_VARIABLE _flashdisk_build_result
    OUTPUT_VARIABLE _flashdisk_build_stdout
    ERROR_VARIABLE _flashdisk_build_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _flashdisk_build_result EQUAL 0)
    message(FATAL_ERROR
        "kernel-flashdisk-includes build failed (${_flashdisk_build_result})\n"
        "${_flashdisk_build_stdout}\n${_flashdisk_build_stderr}")
endif()

string(MAKE_C_IDENTIFIER "kernel-flashdisk" _flashdisk_safe_id)
set(_flashdisk_private_root
    "${_build}/genmodule/arch/riscv-esp32p4/flashdisk/${_flashdisk_safe_id}")
set(_flashdisk_private_include "${_flashdisk_private_root}/include")
set(_flashdisk_headers
    "clib/flashdisk_protos.h"
    "inline/flashdisk.h"
    "defines/flashdisk.h"
    "defines/flashdisk_LVO.h"
    "proto/flashdisk.h")
foreach(_header IN LISTS _flashdisk_headers)
    _compare_reference("${_flashdisk_reference_include}/${_header}"
        "${_flashdisk_private_include}/${_header}")
    foreach(_root IN LISTS _public_roots)
        _compare_reference("${_flashdisk_reference_include}/${_header}"
            "${_root}/${_header}")
    endforeach()
endforeach()
_compare_reference("${_flashdisk_reference_gen}/flashdisk_libdefs.h"
    "${_flashdisk_private_root}/gen/flashdisk_libdefs.h")
_compare_reference("${_flashdisk_reference_fd}/flashdisk_lib.fd"
    "${_flashdisk_private_root}/fd/flashdisk_lib.fd")
_compare_reference("${_flashdisk_reference_fd}/flashdisk_lib.fd"
    "${_build}/SYS/Developer/SDK/fd/flashdisk_lib.fd")

# The override test starts with base AUTO includes frozen off. Despite later
# adding a cdef, functionlist, and alternate include name, neither direct
# genmodule nor the full CMake scaffold may invent headers or an FD; libdefs
# still use the exact merged base-plus-override config.
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target probe-synthetic-device-includes
    RESULT_VARIABLE _synthetic_build_result
    OUTPUT_VARIABLE _synthetic_build_stdout
    ERROR_VARIABLE _synthetic_build_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _synthetic_build_result EQUAL 0)
    message(FATAL_ERROR
        "probe-synthetic-device-includes build failed (${_synthetic_build_result})\n"
        "${_synthetic_build_stdout}\n${_synthetic_build_stderr}")
endif()

string(MAKE_C_IDENTIFIER "probe-synthetic-device" _synthetic_safe_id)
set(_synthetic_private_root
    "${_build}/genmodule/arch/riscv-esp32p4/flashdisk/${_synthetic_safe_id}")
_compare_reference("${_synthetic_reference_gen}/SyntheticDevice_libdefs.h"
    "${_synthetic_private_root}/gen/SyntheticDevice_libdefs.h")
set(_synthetic_header_roots
    "${_synthetic_reference_include}"
    "${_synthetic_private_root}/include" ${_public_roots})
set(_synthetic_header_names
    "clib/override_device_protos.h"
    "inline/override_device.h"
    "defines/override_device.h"
    "defines/override_device_LVO.h"
    "proto/override_device.h"
    "clib/SyntheticDevice_protos.h"
    "inline/SyntheticDevice.h"
    "defines/SyntheticDevice.h"
    "defines/SyntheticDevice_LVO.h"
    "proto/SyntheticDevice.h")
foreach(_root IN LISTS _synthetic_header_roots)
    foreach(_header IN LISTS _synthetic_header_names)
        if(EXISTS "${_root}/${_header}")
            message(FATAL_ERROR
                "base AUTO noincludes choice was lost and produced ${_root}/${_header}")
        endif()
    endforeach()
endforeach()
foreach(_root IN ITEMS
        "${_synthetic_reference_fd}" "${_synthetic_private_root}/fd"
        "${_build}/SYS/Developer/SDK/fd")
    foreach(_name IN ITEMS "override_device_lib.fd" "SyntheticDevice_lib.fd")
        if(EXISTS "${_root}/${_name}")
            message(FATAL_ERROR
                "base AUTO noincludes choice was lost and produced ${_root}/${_name}")
        endif()
    endforeach()
endforeach()
file(READ "${_build}/build.ninja" _device_ninja_graph)
if(_device_ninja_graph MATCHES
   "(override_device|SyntheticDevice)(_protos|_LVO)?[.]h|(override_device|SyntheticDevice)_lib[.]fd")
    message(FATAL_ERROR
        "device override declared outputs excluded by base AUTO noincludes")
endif()

# The second build must be a no-op; then a removed declared header must be
# repaired by the same producer target.
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target probe-kernel-includes
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _noop_result EQUAL 0 OR NOT _noop_stdout MATCHES "no work to do")
    message(FATAL_ERROR
        "resource ABI generation was not a no-op (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()

file(REMOVE "${_build}/SDK/include/inline/kernel.h")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target probe-kernel-includes
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _repair_result EQUAL 0)
    message(FATAL_ERROR
        "resource ABI output repair failed (${_repair_result})\n"
        "${_repair_stdout}\n${_repair_stderr}")
endif()
_compare_all_abi_outputs()

file(REMOVE_RECURSE "${_root}")
message(STATUS "resource module ABI output contract test passed")
