# Test-only bindings for the source-selection admission boundary. The consumer
# fixture mocks only the owned transpiler response; production CMake parsing,
# validation, freshness checks and source/path/digest remeasurement still run.

function(aros_test_bind_native_build_validation source_root contract_path)
    if(NOT IS_DIRECTORY "${source_root}")
        message(FATAL_ERROR "test source-selection root is unavailable: ${source_root}")
    endif()
    file(WRITE "${contract_path}" "{\"schema\":\"aros-native-build-contract-test-v1\"}\n")
    file(REAL_PATH "${source_root}" _validated_source)
    file(REAL_PATH "${contract_path}" _validated_path)
    file(SHA256 "${_validated_path}" _validated_sha256)

    set(AROS_NATIVE_BUILD_CONTRACT "${_validated_path}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CONTRACT_SHA256 "${_validated_sha256}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE PARENT_SCOPE)
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_SOURCE
        "${_validated_source}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_PATH
        "${_validated_path}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_SHA256
        "${_validated_sha256}")
endfunction()

function(aros_test_clear_native_build_validation)
    set(AROS_NATIVE_BUILD_CONTRACT "" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CONTRACT_SHA256 "" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED FALSE PARENT_SCOPE)
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED FALSE)
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_SOURCE "")
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_PATH "")
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_SHA256 "")
endfunction()

function(_aros_test_json_string out_value value)
    string(REPLACE [[\]] [[\\]] _escaped "${value}")
    string(REPLACE [["]] [[\"]] _escaped "${_escaped}")
    set(${out_value} "\"${_escaped}\"" PARENT_SCOPE)
endfunction()

function(_aros_test_shell_quote out_value value)
    string(REPLACE "'" "'\"'\"'" _quoted "${value}")
    set(${out_value} "'${_quoted}'" PARENT_SCOPE)
endfunction()

function(aros_test_prepare_native_consumer_selection source_root contract_path)
    if(NOT IS_DIRECTORY "${source_root}")
        message(FATAL_ERROR "test consumer source root is unavailable: ${source_root}")
    endif()
    file(WRITE "${contract_path}"
        "{\"schema\":\"aros-native-consumer-contract-test-v1\"}\n")
    file(REAL_PATH "${source_root}" _source)
    file(REAL_PATH "${contract_path}" _contract_path)
    file(SHA256 "${_contract_path}" _contract_sha256)

    # This helper is also used with a caller-supplied AROS checkout. Do not
    # create or modify inputs in that tree; bind the mocked response to a
    # source file already present in that tree.
    set(_input_path "")
    foreach(_candidate IN ITEMS
            compiler/startup/startup.c
            workbench/libs/tiff/tif_aros.c
            one.c)
        if(EXISTS "${_source}/${_candidate}" AND
           NOT IS_DIRECTORY "${_source}/${_candidate}")
            set(_input_path "${_candidate}")
            break()
        endif()
    endforeach()
    if(_input_path STREQUAL "")
        message(FATAL_ERROR
            "test consumer source root has no fixture-owned validation input: ${_source}")
    endif()

    if(DEFINED AROS_TARGET_CPU AND NOT "${AROS_TARGET_CPU}" STREQUAL "")
        set(_cpu "${AROS_TARGET_CPU}")
    else()
        set(_cpu riscv32)
    endif()
    if(DEFINED AROS_TARGET_PLATFORM AND NOT "${AROS_TARGET_PLATFORM}" STREQUAL "")
        set(_platform "${AROS_TARGET_PLATFORM}")
    else()
        set(_platform fixture)
    endif()
    set(_profile fixture-native-consumer)
    set(_target_triple "${_cpu}-aros")
    set(_float_abi ilp32f)
    set(_abi_flavour standalone)
    set(_platform_smp OFF)
    set(_use_mmu OFF)

    set(_response "{}")
    _aros_test_json_string(_source_json "${_source}")
    _aros_test_json_string(_contract_path_json "${_contract_path}")
    _aros_test_json_string(_contract_sha256_json "${_contract_sha256}")
    string(JSON _response SET "${_response}" schema
        "\"aros-native-consumer-validation-v1\"")
    string(JSON _response SET "${_response}" qualification
        "\"source-binding-not-graph-or-build-proof\"")
    string(JSON _response SET "${_response}" source_dir "${_source_json}")
    string(JSON _response SET "${_response}" contract_path "${_contract_path_json}")
    string(JSON _response SET "${_response}" contract_sha256 "${_contract_sha256_json}")
    string(JSON _response SET "${_response}" profile "\"${_profile}\"")
    string(JSON _response SET "${_response}" abi "{}")
    string(JSON _response SET "${_response}" abi source_cpu "\"${_cpu}\"")
    string(JSON _response SET "${_response}" abi target_triple "\"${_target_triple}\"")
    string(JSON _response SET "${_response}" abi isa "\"fixture-isa\"")
    string(JSON _response SET "${_response}" abi abi "\"${_float_abi}\"")
    string(JSON _response SET "${_response}" abi code_model "\"medany\"")
    string(JSON _response SET "${_response}" abi flavour "\"${_abi_flavour}\"")
    string(JSON _response SET "${_response}" abi platform_smp false)
    string(JSON _response SET "${_response}" abi use_mmu false)
    string(JSON _response SET "${_response}" exec_smp false)
    string(JSON _response SET "${_response}" sdk_include_relative "\"SYS/Developer/include\"")
    _aros_test_json_string(_input_path_json "${_input_path}")
    string(JSON _response SET "${_response}" input_paths "[]")
    string(JSON _response SET "${_response}" input_paths 0 "${_input_path_json}")

    set(_response_file "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json")
    file(WRITE "${_response_file}" "${_response}\n")
    _aros_test_shell_quote(_response_file_quoted "${_response_file}")
    set(_transpiler "${CMAKE_CURRENT_BINARY_DIR}/mock-aros-transpiler")
    file(WRITE "${_transpiler}" "#!/bin/sh\ncat ${_response_file_quoted}\n")
    file(CHMOD "${_transpiler}" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)

    set(AROS_SOURCE_DIR "${_source}" PARENT_SCOPE)
    aros_test_clear_native_build_validation()
    # aros_test_clear_native_build_validation() runs in this function's scope;
    # propagate the cleared normal variables to its caller as well.
    set(AROS_NATIVE_BUILD_CONTRACT "" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CONTRACT_SHA256 "" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED FALSE PARENT_SCOPE)
    set(AROS_NATIVE_CONSUMER_CONTRACT "${_contract_path}" PARENT_SCOPE)
    set(AROS_NATIVE_CONSUMER_CONTRACT_SHA256 "${_contract_sha256}" PARENT_SCOPE)
    set(AROS_TARGET_PROFILE "${_profile}" PARENT_SCOPE)
    set(AROS_TARGET_CPU "${_cpu}" PARENT_SCOPE)
    set(AROS_TARGET_PLATFORM "${_platform}" PARENT_SCOPE)
    set(AROS_TARGET_TRIPLE "${_target_triple}" PARENT_SCOPE)
    set(AROS_TARGET_FAMILY fixture-family PARENT_SCOPE)
    set(AROS_TARGET_VARIANT fixture-variant PARENT_SCOPE)
    set(AROS_TOOLCHAIN gnu PARENT_SCOPE)
    set(AROS_TARGET_CPU32 fixture-cpu32 PARENT_SCOPE)
    set(GCC_CONFIG_FLOAT_ABI "${_float_abi}" PARENT_SCOPE)
    set(AROS_ABI_FLAVOUR "${_abi_flavour}" PARENT_SCOPE)
    set(AROS_ABI_PLATFORM_SMP "${_platform_smp}" PARENT_SCOPE)
    set(AROS_ENABLE_MMU "${_use_mmu}" PARENT_SCOPE)
    set(AROS_MESA_VERSION fixture-mesa PARENT_SCOPE)
    set(AROS_TRANSPILER_BIN "${_transpiler}" PARENT_SCOPE)
endfunction()
