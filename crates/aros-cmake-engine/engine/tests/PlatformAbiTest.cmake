cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/../BootstrapSDK.cmake")

if(DEFINED INVALID_TARGET)
    aros_platform_abi_config("${INVALID_TARGET}" "pc" _config)
    message(FATAL_ERROR "Unknown platform ABI was accepted")
endif()
if(DEFINED INVALID_DECLARATION)
    set(AROS_ABI_PLATFORM_SMP "${INVALID_DECLARATION}")
    aros_platform_abi_config("declared-cpu" "declared-platform" _config)
    message(FATAL_ERROR "Invalid ABI declaration was accepted")
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

set(AROS_ABI_PLATFORM_SMP OFF)
aros_platform_abi_config("declared-cpu" "declared-platform" _config)
if(NOT _config STREQUAL "")
    message(FATAL_ERROR "Explicit non-SMP public ABI acquired platform padding")
endif()
set(AROS_ABI_PLATFORM_SMP ON)
aros_platform_abi_config("declared-cpu" "declared-platform" _config)
if(NOT _config STREQUAL "#define __AROSPLATFORM_SMP__")
    message(FATAL_ERROR "Explicit platform ABI padding was lost")
endif()
unset(AROS_ABI_PLATFORM_SMP)

foreach(_invalid IN ITEMS "" "false" "TRUE" "ON;unsafe")
    execute_process(COMMAND "${CMAKE_COMMAND}" "-DINVALID_DECLARATION=${_invalid}"
        -P "${CMAKE_CURRENT_LIST_FILE}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _out ERROR_VARIABLE _err)
    if(_result EQUAL 0 OR NOT "${_out}${_err}" MATCHES "must be explicit ON or OFF")
        message(FATAL_ERROR "Invalid public ABI declaration was not rejected: ${_out}${_err}")
    endif()
endforeach()

execute_process(COMMAND "${CMAKE_COMMAND}" -DINVALID_TARGET=unknown
    -P "${CMAKE_CURRENT_LIST_FILE}"
    RESULT_VARIABLE _result OUTPUT_VARIABLE _out ERROR_VARIABLE _err)
if(_result EQUAL 0 OR NOT "${_out}${_err}" MATCHES "No platform ABI contract")
    message(FATAL_ERROR "Unknown target did not fail closed: ${_out}${_err}")
endif()

message(STATUS "Platform ABI padding contracts passed")
