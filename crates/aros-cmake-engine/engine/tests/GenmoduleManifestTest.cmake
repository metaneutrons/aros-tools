cmake_minimum_required(VERSION 3.22)

include("${CMAKE_CURRENT_LIST_DIR}/../GenmoduleManifest.cmake")

get_filename_component(_source_root
    "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)

function(_assert_list_length list_var expected label)
    list(LENGTH ${list_var} _actual)
    if(NOT _actual EQUAL expected)
        message(FATAL_ERROR
            "${label}: expected ${expected} entries, got ${_actual}")
    endif()
endfunction()

function(_assert_list_suffix list_var suffix label)
    foreach(_path IN LISTS ${list_var})
        if(NOT _path MATCHES "${suffix}$")
            message(FATAL_ERROR
                "${label}: '${_path}' does not match /${suffix}")
        endif()
    endforeach()
endfunction()

function(_assert_list_sorted list_var label)
    set(_actual ${${list_var}})
    set(_expected ${_actual})
    list(SORT _expected)
    if(NOT _actual STREQUAL _expected)
        message(FATAL_ERROR
            "${label}: entries do not match GNU Make wildcard order")
    endif()
endfunction()

function(_test_manifest label config module
        expected_total expected_stack expected_regcall)
    set(_root "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-${label}")
    set(_gen_dir "${_root}/gen")
    set(_stub_dir "${_root}/stubs")

    aros_genmodule_writefiles_manifest(_manifest
        CONFIG "${config}"
        MODULE "${module}"
        MODTYPE library
        GEN_DIR "${_gen_dir}"
        STUB_DIR "${_stub_dir}")

    _assert_list_length(_manifest_ALL_OUTPUTS ${expected_total}
        "${label} complete manifest")
    _assert_list_length(_manifest_NORMAL_STACK_STUBS ${expected_stack}
        "${label} normal stack stubs")
    _assert_list_length(_manifest_REL_STACK_STUBS ${expected_stack}
        "${label} relative stack stubs")
    _assert_list_length(_manifest_NORMAL_REGCALL_STUBS ${expected_regcall}
        "${label} normal register stubs")
    _assert_list_length(_manifest_REL_REGCALL_STUBS ${expected_regcall}
        "${label} relative register stubs")
    _assert_list_length(_manifest_NORMAL_AUTOINIT 1
        "${label} normal autoinit")
    _assert_list_length(_manifest_REL_AUTOINIT 1
        "${label} relative autoinit")
    _assert_list_length(_manifest_NORMAL_GETLIBBASE 1
        "${label} normal getlibbase")
    _assert_list_length(_manifest_REL_GETLIBBASE 1
        "${label} relative getlibbase")

    _assert_list_suffix(_manifest_NORMAL_STACK_STUBS "_stub\\.c"
        "${label} normal stack stubs")
    _assert_list_suffix(_manifest_REL_STACK_STUBS "_relstub\\.c"
        "${label} relative stack stubs")
    _assert_list_suffix(_manifest_NORMAL_REGCALL_STUBS "_regcall_stubs\\.c"
        "${label} normal register stubs")
    _assert_list_suffix(_manifest_REL_REGCALL_STUBS "_regcall_relstubs\\.c"
        "${label} relative register stubs")
    _assert_list_suffix(_manifest_NORMAL_AUTOINIT "_autoinit\\.c"
        "${label} normal autoinit")
    _assert_list_suffix(_manifest_REL_AUTOINIT "_relautoinit\\.c"
        "${label} relative autoinit")
    _assert_list_suffix(_manifest_NORMAL_GETLIBBASE "_getlibbase\\.c"
        "${label} normal getlibbase")
    _assert_list_suffix(_manifest_REL_GETLIBBASE "_relgetlibbase\\.c"
        "${label} relative getlibbase")
    _assert_list_sorted(_manifest_NORMAL_STACK_STUBS
        "${label} normal stack wildcard")
    _assert_list_sorted(_manifest_REL_STACK_STUBS
        "${label} relative stack wildcard")
    _assert_list_sorted(_manifest_NORMAL_REGCALL_STUBS
        "${label} normal register wildcard")
    _assert_list_sorted(_manifest_REL_REGCALL_STUBS
        "${label} relative register wildcard")

    # When a freshly built reference tool is supplied, also compare every
    # declared output path with writefiles itself.  The count/name assertions
    # above remain runnable in source-only CI.
    if(DEFINED AROS_HOST_GENMODULE AND AROS_HOST_GENMODULE)
        if(NOT EXISTS "${AROS_HOST_GENMODULE}")
            message(FATAL_ERROR
                "AROS_HOST_GENMODULE does not exist: ${AROS_HOST_GENMODULE}")
        endif()
        file(REMOVE_RECURSE "${_root}")
        file(MAKE_DIRECTORY "${_gen_dir}" "${_stub_dir}")
        execute_process(
            COMMAND "${AROS_HOST_GENMODULE}"
                -c "${config}" -d "${_gen_dir}" -l "${_stub_dir}"
                writefiles "${module}" library
            RESULT_VARIABLE _result
            ERROR_VARIABLE _stderr)
        if(NOT _result EQUAL 0)
            message(FATAL_ERROR
                "${label}: reference genmodule failed (${_result}): ${_stderr}")
        endif()
        file(GLOB_RECURSE _actual LIST_DIRECTORIES FALSE "${_root}/*")
        set(_expected ${_manifest_ALL_OUTPUTS})
        list(SORT _actual)
        list(SORT _expected)
        if(NOT _actual STREQUAL _expected)
            message(FATAL_ERROR
                "${label}: manifest differs from reference genmodule\n"
                "expected: ${_expected}\nactual: ${_actual}")
        endif()
        file(REMOVE_RECURSE "${_root}")
    endif()
endfunction()

# These two declarations exercise both the large GL function surface and the
# declaration-private POSIX LFA variant used by the four restored linklibs.
_test_manifest(gl
    "${_source_root}/workbench/libs/gl/gl.conf" gl 935 463 1)
_test_manifest(posixc_lfa
    "${_source_root}/compiler/crt/posixc/posixc_lfa.conf" posixc 35 13 1)
_test_manifest(zstd
    "${_source_root}/workbench/libs/zstd/zstd.conf" zstd 143 67 1)

aros_genmodule_writefiles_manifest(_z1
    CONFIG "${_source_root}/workbench/libs/z/z1.conf"
    MODULE z1
    MODTYPE library
    GEN_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-z1/gen"
    STUB_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-z1/stubs")
if(NOT _z1_HAS_REL_LINKLIB)
    message(FATAL_ERROR "z1: rellinklib option was not preserved")
endif()
if(NOT _z1_RELLIBS STREQUAL "posixc;stdc")
    message(FATAL_ERROR "z1: expected posixc;stdc rellibs, got ${_z1_RELLIBS}")
endif()
if(NOT _z1_RUNTIME_DEFINES STREQUAL
   "__POSIXC_RELLIBBASE__;__STDC_RELLIBBASE__;__Z1_NOLIBBASE__")
    message(FATAL_ERROR
        "z1: unexpected runtime definitions ${_z1_RUNTIME_DEFINES}")
endif()
if(NOT _z1_LINKLIB_DEFINES STREQUAL
   "__POSIXC_RELLIBBASE__;__STDC_RELLIBBASE__")
    message(FATAL_ERROR
        "z1: unexpected client definitions ${_z1_LINKLIB_DEFINES}")
endif()

aros_genmodule_writefiles_manifest(_zstd
    CONFIG "${_source_root}/workbench/libs/zstd/zstd.conf"
    MODULE zstd
    MODTYPE library
    GEN_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-zstd/gen"
    STUB_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-zstd/stubs")
set(_zstd_normal_archive
    ${_zstd_NORMAL_STUBS}
    ${_zstd_NORMAL_AUTOINIT}
    ${_zstd_NORMAL_GETLIBBASE})
set(_zstd_relative_archive
    ${_zstd_REL_STUBS}
    ${_zstd_REL_AUTOINIT}
    ${_zstd_REL_GETLIBBASE})
_assert_list_length(_zstd_normal_archive 70 "zstd normal client archive")
_assert_list_length(_zstd_relative_archive 70 "zstd relative client archive")
if(NOT _zstd_HAS_REL_LINKLIB)
    message(FATAL_ERROR "zstd: rellinklib option was not preserved")
endif()
if(NOT _zstd_RELLIBS STREQUAL "posixc;stdc")
    message(FATAL_ERROR
        "zstd: expected posixc;stdc rellibs, got ${_zstd_RELLIBS}")
endif()
if(NOT _zstd_RUNTIME_DEFINES STREQUAL
   "__POSIXC_RELLIBBASE__;__STDC_RELLIBBASE__;__ZSTD_NOLIBBASE__")
    message(FATAL_ERROR
        "zstd: unexpected runtime definitions ${_zstd_RUNTIME_DEFINES}")
endif()
if(NOT _zstd_LINKLIB_DEFINES STREQUAL
   "__POSIXC_RELLIBBASE__;__STDC_RELLIBBASE__")
    message(FATAL_ERROR
        "zstd: unexpected client definitions ${_zstd_LINKLIB_DEFINES}")
endif()

message(STATUS "genmodule writefiles manifest test passed")
