cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/../BootstrapSDK.cmake")

if(DEFINED INVALID_TARGET)
    aros_platform_abi_config("${INVALID_TARGET}" "pc" _config)
    message(FATAL_ERROR "Unknown platform ABI was accepted")
endif()

foreach(_pair IN ITEMS "i386/pc" "x86_64/pc" "arm/raspi"
                       "aarch64/raspi" "riscv64/opensbi")
    string(REPLACE "/" ";" _parts "${_pair}")
    list(GET _parts 0 _cpu)
    list(GET _parts 1 _platform)
    aros_platform_abi_config("${_cpu}" "${_platform}" _config)
    if(NOT _config STREQUAL "#define __AROSPLATFORM_SMP__")
        message(FATAL_ERROR "${_pair} lost its public Exec/pthread ABI padding")
    endif()
    if(_config MATCHES "__AROSEXEC_SMP__")
        message(FATAL_ERROR "Platform ABI padding enabled the SMP kernel")
    endif()
endforeach()

execute_process(COMMAND "${CMAKE_COMMAND}" -DINVALID_TARGET=unknown
    -P "${CMAKE_CURRENT_LIST_FILE}"
    RESULT_VARIABLE _result OUTPUT_VARIABLE _out ERROR_VARIABLE _err)
if(_result EQUAL 0 OR NOT "${_out}${_err}" MATCHES "No platform ABI contract")
    message(FATAL_ERROR "Unknown target did not fail closed: ${_out}${_err}")
endif()

message(STATUS "Platform ABI padding contracts passed")
