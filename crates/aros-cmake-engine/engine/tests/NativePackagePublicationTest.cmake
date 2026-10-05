cmake_minimum_required(VERSION 3.22)

if(DEFINED NATIVE_PACKAGE_CONTRACT_CHILD)
    include("${NATIVE_PACKAGE_CONTRACT_MODULE}")
    set(AROS_SOURCE_DIR "${NATIVE_PACKAGE_CONTRACT_SOURCE}")
    set(AROS_NATIVE_BUILD_CONTRACT
        "${NATIVE_PACKAGE_CONTRACT_SOURCE}/native-build-v1.json")
    set(AROS_NATIVE_BUILD_CONTRACT_SHA256 "${NATIVE_PACKAGE_CONTRACT_SHA256}")
    set(AROS_TARGET_PROFILE esp32p4-d1001)
    set(AROS_TARGET_CPU riscv)
    set(AROS_TARGET_TRIPLE riscv-aros)
    set(AROS_TOOLCHAIN gnu)
    set(GCC_CONFIG_FLOAT_ABI ilp32f)
    set(AROS_ABI_FLAVOUR standalone)
    set(AROS_ABI_PLATFORM_SMP OFF)
    set(AROS_ENABLE_MMU OFF)
    set(AROS_GNU_TARGET_COMPILE_OPTIONS
        "-march=rv32imafc_zicsr_zifencei_zaamo_zalrsc;-mabi=ilp32f;-mcmodel=medany;-mstrict-align")
    aros_validate_native_build_contract()
    if(DEFINED NATIVE_PACKAGE_EXPECT_LIMIT)
        if(NOT AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES STREQUAL
           NATIVE_PACKAGE_EXPECT_LIMIT)
            message(FATAL_ERROR
                "native package capacity export differs: '${AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES}'")
        endif()
    elseif(NOT AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES STREQUAL "")
        message(FATAL_ERROR
            "legacy validation-only contract unexpectedly exported a package limit")
    endif()
    return()
endif()

find_program(_ninja NAMES ninja ninja-build)
if(NOT _ninja)
    message(FATAL_ERROR "NativePackagePublicationTest requires Ninja")
endif()

get_filename_component(_engine "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_test_script "${CMAKE_CURRENT_LIST_FILE}")
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/native-package-publication")
set(_contract_fixture "${CMAKE_CURRENT_LIST_DIR}/native-build-contract/source")
set(_contract_module "${_engine}/NativeBuildContract.cmake")
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    file(REAL_PATH "$ENV{TMPDIR}" _temp_root)
else()
    file(REAL_PATH "/tmp" _temp_root)
endif()
set(_root "${_temp_root}/aros-native-package-publication-${_suffix}")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "refusing to reuse native package test root: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

function(_native_package_configure case_name expect_success expected_message out_build)
    set(_build "${_root}/${case_name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DAROS_ENGINE_DIR=${_engine}"
            "-DNATIVE_PACKAGE_CASE=${case_name}"
            ${ARGN}
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 60)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "native package ${case_name} configure failed (${_result})\n${_log}")
    elseif(NOT expect_success AND
           (_result EQUAL 0 OR NOT _log MATCHES "${expected_message}"))
        message(FATAL_ERROR
            "native package ${case_name} was not rejected by '${expected_message}' (${_result})\n${_log}")
    endif()
    set(${out_build} "${_build}" PARENT_SCOPE)
endfunction()

function(_native_package_build build expect_success expected_message)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${build}" --target native-package
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 60)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR "native package build failed (${_result})\n${_log}")
    elseif(NOT expect_success AND
           (_result EQUAL 0 OR NOT _log MATCHES "${expected_message}"))
        message(FATAL_ERROR
            "native package build was not rejected by '${expected_message}' (${_result})\n${_log}")
    endif()
endfunction()

_native_package_configure(exact TRUE "" _exact_build)
_native_package_build("${_exact_build}" TRUE "")
set(_exact_output "${_exact_build}/published.pkg")
if(NOT EXISTS "${_exact_output}" OR IS_DIRECTORY "${_exact_output}")
    message(FATAL_ERROR "exact-limit native package was not published")
endif()
file(SIZE "${_exact_output}" _exact_size)
if(NOT _exact_size EQUAL 5)
    message(FATAL_ERROR "exact-limit package has ${_exact_size} bytes, expected 5")
endif()

_native_package_configure(overflow TRUE "" _overflow_build)
set(_overflow_output "${_overflow_build}/published.pkg")
file(WRITE "${_overflow_output}" "previous published artifact")
_native_package_build("${_overflow_build}" FALSE "above PACKAGE_MAXIMUM_BYTES")
file(READ "${_overflow_output}" _retained_package)
if(NOT _retained_package STREQUAL "previous published artifact")
    message(FATAL_ERROR "oversized package replaced the previous published artifact")
endif()

_native_package_configure(invalid-limit FALSE "positive safe unsigned integer" _unused)
_native_package_configure(absent-limit FALSE "package capacity" _unused)
_native_package_configure(missing-member FALSE "native package is incomplete" _unused
    -DNATIVE_PACKAGE_LIMIT=5)
_native_package_configure(no-present FALSE "native package has no configured members" _unused
    -DNATIVE_PACKAGE_LIMIT=5)

# Keep non-native partial package behavior intact.
_native_package_configure(partial TRUE "" _partial_build)
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_partial_build}" --target nonnative-partial-package
    RESULT_VARIABLE _partial_result OUTPUT_VARIABLE _partial_stdout ERROR_VARIABLE _partial_stderr
    TIMEOUT 60)
if(NOT _partial_result EQUAL 0 OR
   NOT EXISTS "${_partial_build}/published.pkg")
    message(FATAL_ERROR
        "nonnative partial package behavior changed (${_partial_result})\n${_partial_stdout}${_partial_stderr}")
endif()

function(_native_package_contract_case name package_limit expect_success expected_message)
    set(_source "${_root}/contract-${name}")
    file(MAKE_DIRECTORY "${_source}")
    file(COPY "${_contract_fixture}/" DESTINATION "${_source}")
    if(NOT "${package_limit}" STREQUAL "__LEGACY__")
        file(WRITE "${_source}/native-contract-input.txt"
            "{\n  \"package_limit_bytes\": ${package_limit}\n}\n")
        file(SHA256 "${_source}/native-contract-input.txt" _geometry_sha)
        file(READ "${_source}/native-build-v1.json" _contract_json)
        string(JSON _contract_json SET "${_contract_json}" inputs 0 sha256 "\"${_geometry_sha}\"")
        string(JSON _contract_json SET "${_contract_json}" media geometry_contract
            "\"native-contract-input.txt\"")
        file(WRITE "${_source}/native-build-v1.json" "${_contract_json}")
    endif()
    file(SHA256 "${_source}/native-build-v1.json" _contract_sha)
    execute_process(
        COMMAND "${CMAKE_COMMAND}"
            "-DNATIVE_PACKAGE_CONTRACT_CHILD=ON"
            "-DNATIVE_PACKAGE_CONTRACT_MODULE=${_contract_module}"
            "-DNATIVE_PACKAGE_CONTRACT_SOURCE=${_source}"
            "-DNATIVE_PACKAGE_CONTRACT_SHA256=${_contract_sha}"
            ${ARGN}
            -P "${_test_script}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 30)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR "native package contract ${name} failed\n${_log}")
    elseif(NOT expect_success AND
           (_result EQUAL 0 OR NOT _log MATCHES "${expected_message}"))
        message(FATAL_ERROR
            "native package contract ${name} was not rejected by '${expected_message}'\n${_log}")
    endif()
endfunction()

_native_package_contract_case(legacy "__LEGACY__" TRUE "")
_native_package_contract_case(valid 4063232 TRUE ""
    "-DNATIVE_PACKAGE_EXPECT_LIMIT=4063232")
_native_package_contract_case(zero 0 FALSE "package_limit_bytes")
_native_package_contract_case(negative -1 FALSE "package_limit_bytes")
_native_package_contract_case(fraction 1.5 FALSE "package_limit_bytes")
_native_package_contract_case(overflow 9223372036854775808 FALSE "package_limit_bytes")

file(REMOVE_RECURSE "${_root}")
message(STATUS "native package publication and capacity tests passed")
