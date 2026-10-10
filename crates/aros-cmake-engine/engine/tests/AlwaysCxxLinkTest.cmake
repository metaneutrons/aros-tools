cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_build "${_temp_root}/aros-always-cxx-link-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/always-cxx-link")

foreach(_mode IN ITEMS development-no-lld development-lld locked locked-gnu locked-native-gnu locked-consumer-gnu)
    set(_gnu OFF)
    set(_native OFF)
    set(_consumer OFF)
    if(_mode MATCHES "^locked")
        set(_locked ON)
        set(_development_lld OFF)
        if(_mode MATCHES "gnu$")
            set(_gnu ON)
        endif()
        if(_mode STREQUAL "locked-native-gnu" OR _mode STREQUAL "locked-consumer-gnu")
            set(_native ON)
        endif()
        if(_mode STREQUAL "locked-consumer-gnu")
            set(_consumer ON)
        endif()
    elseif(_mode STREQUAL "development-lld")
        set(_locked OFF)
        set(_development_lld ON)
    else()
        set(_locked OFF)
        set(_development_lld OFF)
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}"
            -B "${_build}-${_mode}" -G Ninja
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
            ${AROS_TEST_TOOL_ARGS}
            "-DTEST_LOCKED_TOOLCHAIN=${_locked}"
            "-DTEST_GNU_TOOLCHAIN=${_gnu}"
            "-DTEST_NATIVE_SDK_OBJECTS=${_native}"
            "-DTEST_NATIVE_CONSUMER=${_consumer}"
            "-DTEST_DEVELOPMENT_LLD=${_development_lld}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "always-cxx-link ${_mode} fixture configure failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    endif()
    file(READ "${_build}-${_mode}/mui-runtime-path.txt" _mui_path)
    set(_expected_mui_path "${_build}-${_mode}/SYS/Classes/Zune/IconDrawerList.mui")
    cmake_path(NORMAL_PATH _expected_mui_path)
    if(NOT _mui_path STREQUAL _expected_mui_path)
        message(FATAL_ERROR "MUI loader cannot resolve generated path: ${_mui_path}")
    endif()
    foreach(_header_target IN ITEMS default-headers standard-headers explicit-headers)
        file(READ "${_build}-${_mode}/${_header_target}.includes" _includes)
        file(READ "${_build}-${_mode}/${_header_target}.options" _options)
        if(NOT _includes MATCHES "/aros/stdc")
            message(FATAL_ERROR "${_header_target} lost standard-C headers: ${_includes}")
        endif()
        if(_header_target STREQUAL "standard-headers")
            if(_includes MATCHES "/aros/posixc")
                message(FATAL_ERROR "-noposixc retained implicit POSIX headers: ${_includes}")
            endif()
        elseif(NOT _includes MATCHES "/aros/posixc")
            message(FATAL_ERROR "${_header_target} lost required POSIX headers: ${_includes}")
        endif()
        if(_gnu AND NOT _header_target STREQUAL "default-headers")
            if(NOT _options MATCHES "-noposixc")
                message(FATAL_ERROR "GNU specs lost source header opt-out: ${_options}")
            endif()
        elseif(_options MATCHES "-noposixc")
            message(FATAL_ERROR "runtime header policy leaked to the bare compiler: ${_options}")
        endif()
    endforeach()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}"
        -B "${_build}-consumer-stale-binding" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
        -DTEST_LOCKED_TOOLCHAIN=ON -DTEST_GNU_TOOLCHAIN=ON
        -DTEST_NATIVE_SDK_OBJECTS=ON -DTEST_NATIVE_CONSUMER=ON
        -DTEST_CONSUMER_STALE_BINDING=ON
    RESULT_VARIABLE _stale_result
    OUTPUT_VARIABLE _stale_stdout ERROR_VARIABLE _stale_stderr)
if(_stale_result EQUAL 0 OR
   NOT "${_stale_stdout}\n${_stale_stderr}" MATCHES "contract changed after validation")
    message(FATAL_ERROR
        "stale SDK consumer reached startup producers\n${_stale_stdout}\n${_stale_stderr}")
endif()

function(_native_role_consumer_refusal role expected_message)
    set(_case_build "${_build}-native-missing-${role}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}"
            -B "${_case_build}" -G Ninja
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
            ${AROS_TEST_TOOL_ARGS}
            -DTEST_LOCKED_TOOLCHAIN=ON
            -DTEST_GNU_TOOLCHAIN=OFF
            -DTEST_NATIVE_SDK_OBJECTS=ON
            "-DTEST_NATIVE_MISSING_ROLE=${role}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_log "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT "${_log}" MATCHES "${expected_message}")
        message(FATAL_ERROR
            "native ${role} consumer did not fail at its required role boundary\n${_log}")
    endif()
    message(STATUS "native ${role} consumer rejected missing role with '${expected_message}'")
endfunction()

_native_role_consumer_refusal(C_STARTUP
    "missing-c-startup-consumer: AROS program startup producer is missing")
_native_role_consumer_refusal(C_DETACH
    "missing-detach-consumer: AROS detached startup producer is missing")
_native_role_consumer_refusal(CXX_STARTUP
    "missing-cxx-module: locked C\\+\\+ link has no cxx-startup producer target")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}"
        -B "${_build}-duplicate-runtime" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
        -DTEST_DUPLICATE_RUNTIME_MODULE=ON
    RESULT_VARIABLE _duplicate_result
    OUTPUT_VARIABLE _duplicate_stdout
    ERROR_VARIABLE _duplicate_stderr)
if(_duplicate_result EQUAL 0 OR
   NOT _duplicate_stderr MATCHES "Conflicting runtime module output")
    message(FATAL_ERROR
        "ambiguous runtime module did not fail closed\n"
        "${_duplicate_stdout}\n${_duplicate_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}"
        -B "${_build}-duplicate-hidd" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
        -DTEST_DUPLICATE_HIDD=ON
    RESULT_VARIABLE _hidd_duplicate_result
    OUTPUT_VARIABLE _hidd_duplicate_stdout
    ERROR_VARIABLE _hidd_duplicate_stderr)
if(_hidd_duplicate_result EQUAL 0 OR
   NOT _hidd_duplicate_stderr MATCHES "Conflicting runtime module output")
    message(FATAL_ERROR "ambiguous HIDD did not fail closed\n${_hidd_duplicate_stdout}\n${_hidd_duplicate_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}"
        -B "${_build}-unsupported-detach" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
        -DTEST_UNSUPPORTED_DETACH=ON
    RESULT_VARIABLE _unsupported_result
    OUTPUT_VARIABLE _unsupported_stdout
    ERROR_VARIABLE _unsupported_stderr)
if(_unsupported_result EQUAL 0 OR
   NOT _unsupported_stderr MATCHES "detached startup is not supported for standalone links")
    message(FATAL_ERROR
        "standalone detached declaration did not fail at its capability boundary\n"
        "${_unsupported_stdout}\n${_unsupported_stderr}")
endif()

# The locked fixture deliberately uses its host compiler for this small C
# object, after checking the exact direct-link template separately. Building
# it proves that the linker-visible sysroot file is a concrete output,
# not just a dependency-only placeholder.
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}-locked"
        --target aros-cxx-startup
    RESULT_VARIABLE _startup_result
    OUTPUT_VARIABLE _startup_stdout
    ERROR_VARIABLE _startup_stderr)
if(NOT _startup_result EQUAL 0)
    message(FATAL_ERROR
        "always-cxx-link locked cxx-startup build failed (${_startup_result})\n"
        "${_startup_stdout}\n${_startup_stderr}")
endif()
if(NOT EXISTS "${_build}-locked/SYS/Developer/lib/cxx-startup.o")
    message(FATAL_ERROR
        "always-cxx-link locked fixture did not publish Developer/lib/cxx-startup.o")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}"
        -B "${_build}-gnu-escaped-linker" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
        -DTEST_LOCKED_TOOLCHAIN=ON -DTEST_GNU_TOOLCHAIN=ON
        -DTEST_ESCAPED_GNU_LINKER=ON
    RESULT_VARIABLE _escaped_result
    OUTPUT_VARIABLE _escaped_stdout
    ERROR_VARIABLE _escaped_stderr)
if(_escaped_result EQUAL 0 OR
   NOT _escaped_stderr MATCHES "GNU AROS linker escapes its validated prefix")
    message(FATAL_ERROR
        "GNU module linker accepted a host-side substitution\n"
        "${_escaped_stdout}\n${_escaped_stderr}")
endif()

file(REMOVE_RECURSE
    "${_build}-development-no-lld"
    "${_build}-development-lld"
    "${_build}-locked"
    "${_build}-locked-gnu"
    "${_build}-locked-native-gnu"
    "${_build}-gnu-escaped-linker"
    "${_build}-duplicate-runtime"
    "${_build}-duplicate-hidd"
    "${_build}-unsupported-detach")
foreach(_role IN ITEMS C_STARTUP C_DETACH CXX_STARTUP)
    file(REMOVE_RECURSE "${_build}-native-missing-${_role}")
endforeach()
message(STATUS "always C++ linker contract test passed")
