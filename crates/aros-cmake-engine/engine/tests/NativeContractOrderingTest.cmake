cmake_minimum_required(VERSION 3.22)

# Exercise the real top-level engine, not merely the validation helper.
# A valid contract reaches compiler detection; a bad digest must stop first.
file(REAL_PATH "${CMAKE_CURRENT_LIST_DIR}/.." _engine)
file(REAL_PATH "${CMAKE_CURRENT_LIST_DIR}/native-build-contract/source" _source)
set(_contract "${_source}/native-build-v1.json")
file(SHA256 "${_contract}" _digest)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _nonce)
if(DEFINED ENV{TMPDIR} AND IS_DIRECTORY "$ENV{TMPDIR}")
    file(REAL_PATH "$ENV{TMPDIR}" _temp)
else()
    set(_temp /tmp)
endif()
set(_root "${_temp}/aros-native-order-${_nonce}")
if(EXISTS "${_root}")
    message(FATAL_ERROR "Refusing to reuse native ordering test root")
endif()
file(MAKE_DIRECTORY "${_root}")
set(_arguments
    "-DAROS_SOURCE_DIR=${_source}"
    "-DAROS_NATIVE_BUILD_CONTRACT=${_contract}"
    "-DAROS_TARGET_PROFILE=esp32p4-d1001"
    "-DAROS_TARGET_CPU=riscv"
    "-DAROS_TARGET_TRIPLE=riscv-aros"
    "-DAROS_TOOLCHAIN=gnu"
    "-DGCC_CONFIG_FLOAT_ABI=ilp32f"
    "-DAROS_ABI_FLAVOUR=standalone"
    "-DAROS_ABI_PLATFORM_SMP=OFF"
    "-DAROS_ENABLE_MMU=OFF"
    "-DCMAKE_C_COMPILER=${_root}/deliberately-absent-compiler")

execute_process(COMMAND "${CMAKE_COMMAND}" -S "${_engine}" -B "${_root}/valid"
    ${_arguments} "-DAROS_NATIVE_BUILD_CONTRACT_SHA256=${_digest}"
    RESULT_VARIABLE _valid_result OUTPUT_VARIABLE _valid_out ERROR_VARIABLE _valid_err)
if(_valid_result EQUAL 0 OR
   NOT "${_valid_out}${_valid_err}" MATCHES "CMAKE_C_COMPILER.*|deliberately-absent-compiler" OR
   "${_valid_out}${_valid_err}" MATCHES "Native build contract .*:")
    message(FATAL_ERROR "Valid source contract did not reach compiler detection:\n${_valid_out}${_valid_err}")
endif()
execute_process(COMMAND "${CMAKE_COMMAND}" -S "${_engine}" -B "${_root}/invalid"
    ${_arguments}
    "-DAROS_NATIVE_BUILD_CONTRACT_SHA256=0000000000000000000000000000000000000000000000000000000000000000"
    RESULT_VARIABLE _invalid_result OUTPUT_VARIABLE _invalid_out ERROR_VARIABLE _invalid_err)
if(_invalid_result EQUAL 0 OR
   NOT "${_invalid_out}${_invalid_err}" MATCHES "contract bytes do not match" OR
   "${_invalid_out}${_invalid_err}" MATCHES "deliberately-absent-compiler|compiler identification")
    message(FATAL_ERROR "Invalid contract reached compiler detection or failed for another reason:\n${_invalid_out}${_invalid_err}")
endif()
message(STATUS "Top-level native source validation precedes compiler detection")
