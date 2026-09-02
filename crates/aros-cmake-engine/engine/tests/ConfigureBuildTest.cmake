cmake_minimum_required(VERSION 3.22)

get_filename_component(_repo "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/configure-build")
string(RANDOM LENGTH 10 ALPHABET 0123456789abcdef _suffix)
# Keep the physical-path regression deterministic on macOS, where /tmp is a
# symlink to /private/tmp. The fixture must accept that alias while rejecting
# an actual symlink escape below the build root.
if(CMAKE_HOST_UNIX)
    set(_temp_base "/tmp")
elseif(DEFINED ENV{TEMP} AND NOT "$ENV{TEMP}" STREQUAL "")
    set(_temp_base "$ENV{TEMP}")
else()
    message(FATAL_ERROR "configure-build test needs /tmp or TEMP")
endif()
cmake_path(ABSOLUTE_PATH _temp_base NORMALIZE OUTPUT_VARIABLE _temp_base)
set(_root "${_temp_base}/aros-configure-build-${_suffix}")
file(REMOVE_RECURSE "${_root}")
file(MAKE_DIRECTORY "${_root}")

function(_configure name expect_success expected_message)
    set(_build "${_root}/${name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DAROS_REPO_ROOT=${_repo}"
            "-DCONFIGURE_BUILD_CASE=${name}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "configure-build ${name} configure failed (${_result})\n${_log}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "configure-build ${name} unexpectedly configured")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "configure-build ${name} missed '${expected_message}'\n${_log}")
        endif()
    endif()
    set(CONFIGURED_BUILD "${_build}" PARENT_SCOPE)
endfunction()

_configure(success TRUE "")
set(_build "${CONFIGURED_BUILD}")
file(SHA256 "${_repo}/tools/ADFlib/src/adf_env.c" _source_before)
file(SHA256
    "${_repo}/workbench/network/WirelessManager/wpa_supplicant/main_amiga.c"
    _wireless_source_before)
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target host-adflib
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "configure-build initial build failed (${_build_result})\n${_build_stdout}${_build_stderr}")
endif()
set(_private "${_build}/gen/configure/tools/ADFlib/host/build/libadf.a")
set(_installed "${_build}/hosttools/lib/libadf.a")
foreach(_output IN ITEMS "${_private}" "${_installed}"
        "${_build}/hosttools/include/adflib.h"
        "${_build}/hosttools/include/adf_nativ.h"
        "${_build}/hosttools/lib/pkgconfig/adflib.pc")
    if(NOT EXISTS "${_output}")
        message(FATAL_ERROR "configure-build omitted ${_output}")
    endif()
endforeach()
file(SHA256 "${_private}" _private_sha)
file(SHA256 "${_installed}" _installed_sha)
if(NOT _private_sha STREQUAL _installed_sha)
    message(FATAL_ERROR "installed ADFlib archive differs from its private product")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target host-adflib
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
set(_noop_log "${_noop_stdout}${_noop_stderr}")
if(NOT _noop_result EQUAL 0 OR NOT _noop_log MATCHES "no work to do")
    message(FATAL_ERROR "configure-build second build was not a no-op\n${_noop_log}")
endif()

file(REMOVE "${_installed}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target host-adflib
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr)
if(NOT _repair_result EQUAL 0 OR NOT EXISTS "${_installed}")
    message(FATAL_ERROR
        "configure-build repair failed (${_repair_result})\n${_repair_stdout}${_repair_stderr}")
endif()
file(SHA256 "${_installed}" _repaired_sha)
if(NOT _repaired_sha STREQUAL _installed_sha)
    message(FATAL_ERROR "configure-build repair changed the archive")
endif()

_configure(success TRUE "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${CONFIGURED_BUILD}" --target host-adflib
    RESULT_VARIABLE _reconfigure_result
    OUTPUT_VARIABLE _reconfigure_stdout
    ERROR_VARIABLE _reconfigure_stderr)
set(_reconfigure_log "${_reconfigure_stdout}${_reconfigure_stderr}")
if(NOT _reconfigure_result EQUAL 0 OR
   NOT _reconfigure_log MATCHES "no work to do")
    message(FATAL_ERROR
        "configure-build rebuilt after a no-op reconfigure\n${_reconfigure_log}")
endif()
file(SHA256 "${_repo}/tools/ADFlib/src/adf_env.c" _source_after)
if(NOT _source_before STREQUAL _source_after)
    message(FATAL_ERROR "configure-style runner modified its source tree")
endif()

foreach(_name IN ITEMS host-adflib linklib-adflib workbench-network-wirelessmanager)
    file(READ "${_build}/.aros-${_name}-configure-contract.cmake" _contract)
    if(_name STREQUAL "workbench-network-wirelessmanager")
        if(NOT _contract MATCHES "CB_LINKER")
            message(FATAL_ERROR "WirelessManager contract omitted its linker")
        endif()
    elseif(_contract MATCHES "CB_LINKER")
        message(FATAL_ERROR "${_name} unnecessarily requires a linker")
    endif()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target linklib-adflib
    RESULT_VARIABLE _target_result
    OUTPUT_VARIABLE _target_stdout
    ERROR_VARIABLE _target_stderr)
if(NOT _target_result EQUAL 0)
    message(FATAL_ERROR
        "configure-build target ADFlib failed (${_target_result})\n${_target_stdout}${_target_stderr}")
endif()
foreach(_output IN ITEMS
        "${_build}/gen/configure/tools/ADFlib/target/build/libadf.a"
        "${_build}/SYS/Developer/lib/libadf.a"
        "${_build}/SYS/Developer/include/adflib.h"
        "${_build}/SYS/Developer/lib/pkgconfig/adflib.pc")
    if(NOT EXISTS "${_output}")
        message(FATAL_ERROR "target ADFlib omitted ${_output}")
    endif()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target workbench-network-wirelessmanager
    RESULT_VARIABLE _wireless_result
    OUTPUT_VARIABLE _wireless_stdout
    ERROR_VARIABLE _wireless_stderr)
if(NOT _wireless_result EQUAL 0)
    message(FATAL_ERROR
        "configure-build WirelessManager failed (${_wireless_result})\n${_wireless_stdout}${_wireless_stderr}")
endif()
foreach(_output IN ITEMS
        "${_build}/liblinklibs-mui.a"
        "${_build}/gen/configure/workbench/network/WirelessManager/source/wpa_supplicant/wpa_supplicant"
        "${_build}/gen/configure/workbench/network/WirelessManager/source/wpa_supplicant/wpa_passphrase"
        "${_build}/gen/configure/workbench/network/WirelessManager/source/wpa_supplicant/wpa_cli"
        "${_build}/SYS/C/WirelessManager")
    if(NOT EXISTS "${_output}")
        message(FATAL_ERROR "WirelessManager omitted ${_output}")
    endif()
endforeach()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target workbench-network-wirelessmanager
    RESULT_VARIABLE _wireless_noop_result
    OUTPUT_VARIABLE _wireless_noop_stdout
    ERROR_VARIABLE _wireless_noop_stderr)
set(_wireless_noop_log "${_wireless_noop_stdout}${_wireless_noop_stderr}")
if(NOT _wireless_noop_result EQUAL 0 OR
   NOT _wireless_noop_log MATCHES "no work to do")
    message(FATAL_ERROR
        "WirelessManager second build was not a no-op\n${_wireless_noop_log}")
endif()
file(SHA256
    "${_repo}/workbench/network/WirelessManager/wpa_supplicant/main_amiga.c"
    _wireless_source_after)
if(NOT _wireless_source_before STREQUAL _wireless_source_after)
    message(FATAL_ERROR "configure-style runner modified WirelessManager sources")
endif()

_configure(bad-inventory FALSE "missing or escaped configure input missing.c")
_configure(escape-binary FALSE "binary directory must be a private child")
_configure(wrong-identity FALSE "target identity differs from the audited")
_configure(symlink-binary FALSE "configure root escapes the build tree")
_configure(symlink-input FALSE "missing or escaped configure input src/adf_env.c")

file(REMOVE_RECURSE "${_root}")
message(STATUS "configure-style build test passed")
