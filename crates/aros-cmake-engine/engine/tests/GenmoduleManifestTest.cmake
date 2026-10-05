cmake_minimum_required(VERSION 3.22)

if(AROS_GENMODULE_MANIFEST_UNSUPPORTED_PROBE OR AROS_GENMODULE_MANIFEST_INCLUDE_PROBE)
    include("${AROS_GENMODULE_MANIFEST_MODULE}")
    aros_genmodule_writefiles_manifest(_unsupported
        CONFIG "${AROS_GENMODULE_MANIFEST_CONFIG}"
        MODULE unsupported
        MODTYPE "${AROS_GENMODULE_MANIFEST_PROBE_TYPE}"
        GEN_DIR "${AROS_GENMODULE_MANIFEST_GEN_DIR}"
        STUB_DIR "${AROS_GENMODULE_MANIFEST_STUB_DIR}")
    message(FATAL_ERROR "invalid manifest probe unexpectedly succeeded")
endif()

include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

include("${CMAKE_CURRENT_LIST_DIR}/../GenmoduleManifest.cmake")

set(_source_root "${AROS_TEST_TREE}")

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
    set(_config_override "${ARGN}")

    set(_manifest_args
        CONFIG "${config}"
        MODULE "${module}"
        MODTYPE library
        GEN_DIR "${_gen_dir}"
        STUB_DIR "${_stub_dir}")
    if(_config_override)
        list(APPEND _manifest_args CONFIG_OVERRIDE "${_config_override}")
    endif()
    aros_genmodule_writefiles_manifest(_manifest ${_manifest_args})

    if(NOT _manifest_HAS_INCLUDES STREQUAL "ON")
        message(FATAL_ERROR "${label}: library default includes policy was not ON")
    endif()
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
        set(_reference_command "${AROS_HOST_GENMODULE}")
        if(_config_override)
            list(APPEND _reference_command -o "${_config_override}")
        endif()
        list(APPEND _reference_command
            -c "${config}" -d "${_gen_dir}" -l "${_stub_dir}"
            writefiles "${module}" library)
        execute_process(
            COMMAND ${_reference_command}
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
_test_manifest(mesa_override
    "${_source_root}/workbench/libs/gl/gl.conf" mesa3dgl26-0 935 463 1
    "${_source_root}/workbench/libs/mesa/mesa3dgl.conf")

aros_genmodule_writefiles_manifest(_resource_auto_on
    CONFIG "${_source_root}/rom/task/task.conf"
    MODULE Task
    MODTYPE resource
    GEN_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-resource/gen"
    STUB_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-resource/stubs")
if(NOT _resource_auto_on_HAS_INCLUDES STREQUAL "ON")
    message(FATAL_ERROR "resource default includes policy was not ON")
endif()

aros_genmodule_writefiles_manifest(_mcc_auto_on
    CONFIG "${_source_root}/workbench/libs/gl/gl.conf"
    MODULE GL
    MODTYPE mcc
    GEN_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-mcc/gen"
    STUB_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-mcc/stubs")
if(NOT _mcc_auto_on_HAS_INCLUDES STREQUAL "ON")
    message(FATAL_ERROR "MCC function/cdef default includes policy was not ON")
endif()

set(_miami_manifest_root
    "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-miami")
aros_genmodule_writefiles_manifest(_miami
    CONFIG "${_source_root}/workbench/network/stacks/AROSTCP/MUI.MiamiPanel/MUI.MiamiPanel.conf"
    MODULE MUI
    MODTYPE library
    GEN_DIR "${_miami_manifest_root}/gen"
    STUB_DIR "${_miami_manifest_root}/stubs")
if(NOT _miami_HAS_INCLUDES STREQUAL "OFF")
    message(FATAL_ERROR "MUI.MiamiPanel: noincludes was not preserved")
endif()
_assert_list_length(_miami_NORMAL_GETLIBBASE 1
    "MUI.MiamiPanel getlibbase despite noincludes")
_assert_list_length(_miami_NORMAL_STUBS 0 "MUI.MiamiPanel nostubs")
_assert_list_length(_miami_NORMAL_AUTOINIT 0 "MUI.MiamiPanel noautoinit")
_assert_list_length(_miami_ALL_OUTPUTS 4 "MUI.MiamiPanel writefiles manifest")

aros_genmodule_writefiles_manifest(_mui_auto_off
    CONFIG "${_source_root}/workbench/libs/muimaster/classes/palette.conf"
    MODULE Palette
    MODTYPE mui
    GEN_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-mui-auto/gen"
    STUB_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-mui-auto/stubs")
if(NOT _mui_auto_off_HAS_INCLUDES STREQUAL "OFF")
    message(FATAL_ERROR
        "MUI class with only private definitions/methods should default to no includes")
endif()

set(_unsupported_probe_root
    "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-unsupported")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DAROS_GENMODULE_MANIFEST_UNSUPPORTED_PROBE=ON"
        "-DAROS_GENMODULE_MANIFEST_PROBE_TYPE=hook"
        "-DAROS_GENMODULE_MANIFEST_MODULE=${CMAKE_CURRENT_LIST_DIR}/../GenmoduleManifest.cmake"
        "-DAROS_GENMODULE_MANIFEST_CONFIG=${_source_root}/workbench/libs/gl/gl.conf"
        "-DAROS_GENMODULE_MANIFEST_GEN_DIR=${_unsupported_probe_root}/gen"
        "-DAROS_GENMODULE_MANIFEST_STUB_DIR=${_unsupported_probe_root}/stubs"
        -P "${CMAKE_CURRENT_LIST_FILE}"
    RESULT_VARIABLE _unsupported_result
    OUTPUT_VARIABLE _unsupported_stdout
    ERROR_VARIABLE _unsupported_stderr)
if(_unsupported_result EQUAL 0)
    message(FATAL_ERROR "unsupported genmodule type unexpectedly resolved AUTO includes")
endif()
set(_unsupported_log "${_unsupported_stdout}\n${_unsupported_stderr}")
string(FIND "${_unsupported_log}"
    "cannot resolve automatic include policy" _unsupported_diagnostic)
if(_unsupported_diagnostic LESS 0)
    message(FATAL_ERROR
        "unsupported genmodule type failed without a clear AUTO-includes diagnostic:\n"
        "${_unsupported_log}")
endif()

if(DEFINED AROS_HOST_GENMODULE AND AROS_HOST_GENMODULE)
    set(_miami_config
        "${_source_root}/workbench/network/stacks/AROSTCP/MUI.MiamiPanel/MUI.MiamiPanel.conf")
    set(_miami_root
        "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-miami-reference")
    set(_miami_gen "${_miami_root}/gen")
    set(_miami_stubs "${_miami_root}/stubs")
    set(_miami_includes "${_miami_root}/includes")
    set(_miami_fd "${_miami_root}/fd")
    file(REMOVE_RECURSE "${_miami_root}")
    file(MAKE_DIRECTORY "${_miami_gen}" "${_miami_stubs}"
        "${_miami_includes}" "${_miami_fd}")
    execute_process(
        COMMAND "${AROS_HOST_GENMODULE}" -c "${_miami_config}"
            -d "${_miami_gen}" -l "${_miami_stubs}"
            writefiles MUI library
        RESULT_VARIABLE _miami_writefiles_result
        ERROR_VARIABLE _miami_writefiles_stderr)
    if(NOT _miami_writefiles_result EQUAL 0)
        message(FATAL_ERROR
            "MUI.MiamiPanel reference writefiles failed: ${_miami_writefiles_stderr}")
    endif()
    file(GLOB_RECURSE _miami_actual LIST_DIRECTORIES FALSE "${_miami_root}/*")
    set(_miami_expected "")
    foreach(_expected_path IN LISTS _miami_ALL_OUTPUTS)
        file(RELATIVE_PATH _expected_relative
            "${_miami_manifest_root}" "${_expected_path}")
        list(APPEND _miami_expected "${_miami_root}/${_expected_relative}")
    endforeach()
    list(SORT _miami_actual)
    list(SORT _miami_expected)
    if(NOT _miami_actual STREQUAL _miami_expected)
        message(FATAL_ERROR
            "MUI.MiamiPanel noincludes writefiles mismatch\n"
            "expected: ${_miami_expected}\nactual: ${_miami_actual}")
    endif()

    execute_process(
        COMMAND "${AROS_HOST_GENMODULE}" -c "${_miami_config}"
            -d "${_miami_includes}" writeincludes MUI library
        RESULT_VARIABLE _miami_includes_result
        ERROR_VARIABLE _miami_includes_stderr)
    if(NOT _miami_includes_result EQUAL 0)
        message(FATAL_ERROR
            "MUI.MiamiPanel reference writeincludes failed: ${_miami_includes_stderr}")
    endif()
    file(GLOB_RECURSE _miami_include_outputs LIST_DIRECTORIES FALSE
        "${_miami_includes}/*")
    if(_miami_include_outputs)
        message(FATAL_ERROR
            "MUI.MiamiPanel noincludes produced public headers: ${_miami_include_outputs}")
    endif()

    execute_process(
        COMMAND "${AROS_HOST_GENMODULE}" -c "${_miami_config}"
            -d "${_miami_fd}" writefd MUI library
        RESULT_VARIABLE _miami_fd_result
        ERROR_VARIABLE _miami_fd_stderr)
    if(NOT _miami_fd_result EQUAL 0)
        message(FATAL_ERROR
            "MUI.MiamiPanel reference writefd failed: ${_miami_fd_stderr}")
    endif()
    if(EXISTS "${_miami_fd}/MUI_lib.fd")
        message(FATAL_ERROR "MUI.MiamiPanel noincludes produced an FD file")
    endif()
    file(REMOVE_RECURSE "${_miami_root}")
endif()

aros_genmodule_writefiles_manifest(_mesa_override
    CONFIG "${_source_root}/workbench/libs/gl/gl.conf"
    CONFIG_OVERRIDE "${_source_root}/workbench/libs/mesa/mesa3dgl.conf"
    MODULE mesa3dgl26-0
    MODTYPE library
    GEN_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-mesa-facts/gen"
    STUB_DIR "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-manifest-mesa-facts/stubs")
if(NOT _mesa_override_RELLIBS STREQUAL "z1;posixc;stdc")
    message(FATAL_ERROR
        "Mesa override rellib facts mismatch: ${_mesa_override_RELLIBS}")
endif()
list(SUBLIST _mesa_override_RUNTIME_DEFINES 0 3 _mesa_runtime_rellib_defines)
if(NOT _mesa_override_HAS_REL_LINKLIB OR
   NOT _mesa_runtime_rellib_defines STREQUAL
       "__Z1_RELLIBBASE__;__POSIXC_RELLIBBASE__;__STDC_RELLIBBASE__" OR
   NOT _mesa_override_LINKLIB_DEFINES STREQUAL
       "__Z1_RELLIBBASE__;__POSIXC_RELLIBBASE__;__STDC_RELLIBBASE__")
    message(FATAL_ERROR
        "Mesa override base-selection facts mismatch: "
        "${_mesa_override_RUNTIME_DEFINES} / ${_mesa_override_LINKLIB_DEFINES}")
endif()

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

set(_name_probe_root "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/genmodule-include-name")
file(MAKE_DIRECTORY "${_name_probe_root}")
set(_name_base "${_name_probe_root}/base.conf")
set(_name_override "${_name_probe_root}/override.conf")
file(WRITE "${_name_base}"
    "##begin config\nincludename base_headers\n##end config\n")
file(WRITE "${_name_override}"
    "##begin config\nincludename override_headers\n##end config\n"
    "##begin functionlist\nLONG Example(void)\n##end functionlist\n")
aros_genmodule_writefiles_manifest(_name_base_manifest
    CONFIG "${_name_base}" MODULE name_probe MODTYPE resource
    GEN_DIR "${_name_probe_root}/gen" STUB_DIR "${_name_probe_root}/stubs")
if(NOT _name_base_manifest_INCLUDE_NAME STREQUAL "base_headers" OR
   _name_base_manifest_HAS_FUNCTIONS)
    message(FATAL_ERROR "base include-name/function facts are not source-equivalent")
endif()
aros_genmodule_writefiles_manifest(_name_override_manifest
    CONFIG "${_name_base}" CONFIG_OVERRIDE "${_name_override}"
    MODULE name_probe MODTYPE resource
    GEN_DIR "${_name_probe_root}/gen" STUB_DIR "${_name_probe_root}/stubs")
if(NOT _name_override_manifest_INCLUDE_NAME STREQUAL "override_headers" OR
   NOT _name_override_manifest_HAS_FUNCTIONS)
    message(FATAL_ERROR "override include-name/function facts were not applied")
endif()
if(NOT _zstd_INCLUDE_NAME STREQUAL "zstd" OR NOT _zstd_HAS_FUNCTIONS)
    message(FATAL_ERROR "default include-name/function facts are not source-equivalent")
endif()
foreach(_unsafe "../escape" "path/name" "name;extra" "name extra" "\${escape}")
    file(WRITE "${_name_base}"
        "##begin config\nincludename ${_unsafe}\n##end config\n")
    execute_process(COMMAND "${CMAKE_COMMAND}"
        -DAROS_GENMODULE_MANIFEST_INCLUDE_PROBE=ON
        -DAROS_GENMODULE_MANIFEST_PROBE_TYPE=resource
        "-DAROS_GENMODULE_MANIFEST_MODULE=${CMAKE_CURRENT_LIST_DIR}/../GenmoduleManifest.cmake"
        "-DAROS_GENMODULE_MANIFEST_CONFIG=${_name_base}"
        "-DAROS_GENMODULE_MANIFEST_GEN_DIR=${_name_probe_root}/gen"
        "-DAROS_GENMODULE_MANIFEST_STUB_DIR=${_name_probe_root}/stubs"
        -P "${CMAKE_CURRENT_LIST_FILE}"
        RESULT_VARIABLE _unsafe_result OUTPUT_VARIABLE _unsafe_stdout
        ERROR_VARIABLE _unsafe_stderr TIMEOUT 30)
    if(_unsafe_result EQUAL 0 OR
       NOT "${_unsafe_stdout}\n${_unsafe_stderr}" MATCHES "unsafe genmodule includename")
        message(FATAL_ERROR "unsafe includename '${_unsafe}' was not refused: ${_unsafe_stderr}")
    endif()
endforeach()
file(REMOVE_RECURSE "${_name_probe_root}")

message(STATUS "genmodule writefiles manifest test passed")
