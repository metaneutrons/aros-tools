# A compiler/SDK consumer is not a core, package or boot-media build.
# The owned Rust loader validates the closed, bounded source document and
# every input before project() invokes a compiler. CMake only consumes its
# measured binding and ABI; later graph passes repeat the exact selection.
include_guard(GLOBAL)
include("${CMAKE_CURRENT_LIST_DIR}/NativeBuildContract.cmake")

function(_aros_native_consumer_fail reason)
    message(FATAL_ERROR "Native consumer contract: ${reason}")
endfunction()

function(aros_validate_native_consumer_contract)
    foreach(_required IN ITEMS AROS_NATIVE_CONSUMER_CONTRACT
            AROS_NATIVE_CONSUMER_CONTRACT_SHA256 AROS_SOURCE_DIR AROS_TARGET_PROFILE
            AROS_TARGET_CPU AROS_TARGET_PLATFORM AROS_TARGET_TRIPLE AROS_TOOLCHAIN
            GCC_CONFIG_FLOAT_ABI AROS_ABI_FLAVOUR AROS_ABI_PLATFORM_SMP AROS_ENABLE_MMU)
        if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
            _aros_native_consumer_fail("${_required} must be provided")
        endif()
    endforeach()
    # Empty selectors are explicit values, not omitted command arguments.
    foreach(_required IN ITEMS AROS_TARGET_FAMILY AROS_TARGET_VARIANT AROS_TARGET_CPU32)
        if(NOT DEFINED ${_required})
            _aros_native_consumer_fail("${_required} must be explicitly provided, possibly empty")
        endif()
    endforeach()
    if(AROS_NATIVE_BUILD_CONTRACT)
        _aros_native_consumer_fail("build and consumer selections are mutually exclusive")
    endif()
    _aros_native_validate_sha256("${AROS_NATIVE_CONSUMER_CONTRACT_SHA256}" "consumer digest")
    if(NOT AROS_TOOLCHAIN STREQUAL "gnu")
        _aros_native_consumer_fail("requires the explicit GNU toolchain")
    endif()
    if(NOT IS_ABSOLUTE "${AROS_NATIVE_CONSUMER_CONTRACT}")
        _aros_native_consumer_fail("contract path must be absolute")
    endif()
    file(REAL_PATH "${AROS_SOURCE_DIR}" _source)
    file(REAL_PATH "${AROS_NATIVE_CONSUMER_CONTRACT}" _contract)
    if(NOT _contract STREQUAL AROS_NATIVE_CONSUMER_CONTRACT)
        _aros_native_consumer_fail("contract path must already be canonical")
    endif()
    _aros_native_selector_boolean(_smp "${AROS_ABI_PLATFORM_SMP}" "AROS_ABI_PLATFORM_SMP")
    _aros_native_selector_boolean(_mmu "${AROS_ENABLE_MMU}" "AROS_ENABLE_MMU")
    if(_mmu)
        set(_mmu_arg 1)
    else()
        set(_mmu_arg 0)
    endif()
    set(_transpiler "${AROS_TRANSPILER_BIN}")
    if(NOT _transpiler AND AROS_RUST_TOOLS_DIR)
        set(_transpiler "${AROS_RUST_TOOLS_DIR}/aros-transpiler")
    endif()
    if(NOT IS_ABSOLUTE "${_transpiler}" OR NOT EXISTS "${_transpiler}" OR
       IS_DIRECTORY "${_transpiler}")
        _aros_native_consumer_fail("an explicit absolute aros-transpiler executable is required before project()")
    endif()
    execute_process(COMMAND "${_transpiler}"
        --validate-native-consumer-only
        --native-consumer-profile "${AROS_TARGET_PROFILE}"
        --native-consumer-contract-sha256 "${AROS_NATIVE_CONSUMER_CONTRACT_SHA256}"
        --source-dir "${_source}"
        --cpu "${AROS_TARGET_CPU}" --platform "${AROS_TARGET_PLATFORM}"
        --family "${AROS_TARGET_FAMILY}" --variant "${AROS_TARGET_VARIANT}"
        --toolchain "${AROS_TOOLCHAIN}" --cpu32 "${AROS_TARGET_CPU32}"
        --use-mmu "${_mmu_arg}" --float-abi "${GCC_CONFIG_FLOAT_ABI}"
        --mesa-version "${AROS_MESA_VERSION}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _json ERROR_VARIABLE _error
        TIMEOUT 60)
    if(NOT _result STREQUAL "0")
        _aros_native_consumer_fail("source binding validation failed (${_result}): ${_error}")
    endif()
    string(LENGTH "${_json}" _length)
    if(_length GREATER 1048576)
        _aros_native_consumer_fail("validation response exceeds its closed limit")
    endif()
    _aros_native_require_object_members("${_json}" "consumer response"
        "schema;qualification;source_dir;contract_path;contract_sha256;profile;abi;exec_smp;input_paths;sdk_include_relative")
    foreach(_name IN ITEMS schema qualification source_dir contract_path contract_sha256 profile sdk_include_relative)
        _aros_native_get_string(_${_name} "${_json}" "consumer response" "${_name}")
    endforeach()
    if(NOT _schema STREQUAL "aros-native-consumer-validation-v1" OR
       NOT _qualification STREQUAL "source-binding-not-graph-or-build-proof" OR
       NOT _source_dir STREQUAL _source OR NOT _contract_path STREQUAL _contract OR
       NOT _contract_sha256 STREQUAL AROS_NATIVE_CONSUMER_CONTRACT_SHA256 OR
       NOT _profile STREQUAL AROS_TARGET_PROFILE)
        _aros_native_consumer_fail("validation response differs from the requested binding")
    endif()
    _aros_native_validate_source_path(_sdk_include_relative "${_sdk_include_relative}"
        "consumer SDK include root")
    _aros_native_require_type("${_json}" "consumer response" abi OBJECT)
    string(JSON _abi GET "${_json}" abi)
    _aros_native_require_object_members("${_abi}" "consumer ABI"
        "source_cpu;target_triple;isa;abi;code_model;flavour;platform_smp;use_mmu")
    foreach(_name IN ITEMS source_cpu target_triple isa abi code_model flavour)
        _aros_native_get_string(_abi_${_name} "${_abi}" "consumer ABI" "${_name}")
        _aros_native_validate_token("${_abi_${_name}}" "consumer ABI ${_name}")
    endforeach()
    foreach(_name IN ITEMS platform_smp use_mmu)
        _aros_native_require_type("${_abi}" "consumer ABI" "${_name}" BOOLEAN)
        string(JSON _abi_${_name} GET "${_abi}" "${_name}")
    endforeach()
    _aros_native_require_type("${_json}" "consumer response" exec_smp BOOLEAN)
    string(JSON _exec_smp GET "${_json}" exec_smp)
    if(NOT _abi_source_cpu STREQUAL AROS_TARGET_CPU OR
       NOT _abi_target_triple STREQUAL AROS_TARGET_TRIPLE OR
       NOT _abi_abi STREQUAL GCC_CONFIG_FLOAT_ABI OR
       NOT _abi_flavour STREQUAL AROS_ABI_FLAVOUR OR
       NOT _abi_platform_smp STREQUAL _smp OR NOT _abi_use_mmu STREQUAL _mmu)
        _aros_native_consumer_fail("source ABI differs from the requested configuration")
    endif()
    # The loader used a no-follow bounded read. Recheck the exact raw binding
    # after process return; neither identity locks nor compiler checks may use
    # an earlier response after this document changes.
    file(SHA256 "${_contract}" _digest)
    if(NOT _digest STREQUAL _contract_sha256)
        _aros_native_consumer_fail("contract changed during validation")
    endif()
    # An incremental build must reconfigure when sealed source inputs change,
    # not continue using a previously selected graph without revalidation.
    _aros_native_require_type("${_json}" "consumer response" input_paths ARRAY)
    string(JSON _input_count LENGTH "${_json}" input_paths)
    if(_input_count LESS 1 OR _input_count GREATER 128)
        _aros_native_consumer_fail("validation response has invalid input bounds")
    endif()
    set(_configure_inputs "${_contract}")
    math(EXPR _last_input "${_input_count} - 1")
    foreach(_index RANGE 0 ${_last_input})
        string(JSON _type TYPE "${_json}" input_paths ${_index})
        if(NOT _type STREQUAL "STRING")
            _aros_native_consumer_fail("validation input path must be a string")
        endif()
        string(JSON _input GET "${_json}" input_paths ${_index})
        _aros_native_validate_source_path(_input "${_input}" "consumer input path")
        list(APPEND _configure_inputs "${_source}/${_input}")
    endforeach()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${_configure_inputs}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SOURCE_DIR "${_source}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_PATH "${_contract}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SHA256 "${_digest}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_PROFILE "${_profile}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_ISA "${_abi_isa}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_ABI "${_abi_abi}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_CODE_MODEL "${_abi_code_model}")
    foreach(_selector IN ITEMS AROS_TARGET_CPU AROS_TARGET_PLATFORM AROS_TARGET_TRIPLE
            AROS_TARGET_FAMILY AROS_TARGET_VARIANT AROS_TOOLCHAIN AROS_TARGET_CPU32
            GCC_CONFIG_FLOAT_ABI AROS_ABI_FLAVOUR AROS_MESA_VERSION)
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_${_selector} "${${_selector}}")
    endforeach()
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_PLATFORM_SMP "${_smp}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_USE_MMU "${_mmu}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_EXEC_SMP "${_exec_smp}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SDK_INCLUDE_RELATIVE "${_sdk_include_relative}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATION_CURRENT TRUE)
    set(AROS_NATIVE_CONSUMER_PROFILE "${_profile}" PARENT_SCOPE)
    set(AROS_NATIVE_CONSUMER_EXEC_SMP "${_exec_smp}" PARENT_SCOPE)
    set(AROS_NATIVE_CONSUMER_SDK_INCLUDE_RELATIVE "${_sdk_include_relative}" PARENT_SCOPE)
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE PARENT_SCOPE)
    set(AROS_TRANSPILER_BIN "${_transpiler}" PARENT_SCOPE)
endfunction()

function(_aros_native_consumer_current out_source out_path out_digest)
    get_property(_current GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATION_CURRENT)
    if(NOT _current)
        _aros_native_consumer_fail("requires fresh validation in this configure process")
    endif()
    foreach(_key IN ITEMS SOURCE_DIR PATH SHA256 PROFILE)
        get_property(_${_key} GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_${_key})
    endforeach()
    file(REAL_PATH "${AROS_SOURCE_DIR}" _source)
    if(NOT _source STREQUAL _SOURCE_DIR OR NOT AROS_NATIVE_CONSUMER_CONTRACT STREQUAL _PATH OR
       NOT AROS_NATIVE_CONSUMER_CONTRACT_SHA256 STREQUAL _SHA256 OR
       NOT AROS_TARGET_PROFILE STREQUAL _PROFILE)
        _aros_native_consumer_fail("configuration changed after validation")
    endif()
    file(SHA256 "${_PATH}" _actual)
    if(NOT _actual STREQUAL _SHA256)
        _aros_native_consumer_fail("contract changed after validation")
    endif()
    foreach(_selector IN ITEMS AROS_TARGET_CPU AROS_TARGET_PLATFORM AROS_TARGET_TRIPLE
            AROS_TARGET_FAMILY AROS_TARGET_VARIANT AROS_TOOLCHAIN AROS_TARGET_CPU32
            GCC_CONFIG_FLOAT_ABI AROS_ABI_FLAVOUR AROS_MESA_VERSION)
        get_property(_selected GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_${_selector})
        if(NOT "${${_selector}}" STREQUAL "${_selected}")
            _aros_native_consumer_fail("${_selector} changed after validation")
        endif()
    endforeach()
    _aros_native_selector_boolean(_smp "${AROS_ABI_PLATFORM_SMP}" "AROS_ABI_PLATFORM_SMP")
    _aros_native_selector_boolean(_mmu "${AROS_ENABLE_MMU}" "AROS_ENABLE_MMU")
    get_property(_selected_smp GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_PLATFORM_SMP)
    get_property(_selected_mmu GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_USE_MMU)
    get_property(_selected_exec_smp GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_EXEC_SMP)
    get_property(_selected_sdk_include GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SDK_INCLUDE_RELATIVE)
    if(NOT _smp STREQUAL _selected_smp OR NOT _mmu STREQUAL _selected_mmu OR
       NOT AROS_NATIVE_CONSUMER_EXEC_SMP STREQUAL _selected_exec_smp OR
       NOT AROS_NATIVE_CONSUMER_PROFILE STREQUAL _PROFILE OR
       NOT AROS_NATIVE_CONSUMER_SDK_INCLUDE_RELATIVE STREQUAL _selected_sdk_include)
        _aros_native_consumer_fail("ABI or consumer selectors changed after validation")
    endif()
    set(${out_source} "${_SOURCE_DIR}" PARENT_SCOPE)
    set(${out_path} "${_PATH}" PARENT_SCOPE)
    set(${out_digest} "${_SHA256}" PARENT_SCOPE)
endfunction()

# One header root for bootstrap, source producers and target compilation.
# Ordinary nonconsumer builds retain their historical internal SDK layout.
# An explicitly selected compiler consumer must export the source-declared
# headers beneath its Developer sysroot, where a normal GNU driver finds them.
function(aros_resolve_sdk_include_root out_root)
    if(AROS_NATIVE_CONSUMER_CONTRACT OR AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED)
        _aros_native_consumer_current(_source _path _digest)
        set(_root "${CMAKE_BINARY_DIR}/${AROS_NATIVE_CONSUMER_SDK_INCLUDE_RELATIVE}")
        if(NOT DEFINED AROS_DEVELOPER_INCLUDE_DIR OR
           NOT _root STREQUAL AROS_DEVELOPER_INCLUDE_DIR)
            _aros_native_consumer_fail("source SDK include root differs from the Developer sysroot layout")
        endif()
    else()
        set(_root "${CMAKE_BINARY_DIR}/SDK/include")
    endif()
    set(${out_root} "${_root}" PARENT_SCOPE)
endfunction()

# Generic source-selected SDK producers accept either native selection, but
# never promote a consumer selection into core/package/media build authority.
# Revalidate the original configure-process binding at every admission point;
# a cached boolean alone is not proof of a selected source graph.
function(_aros_native_source_selection_validated out_validated)
    if(AROS_NATIVE_CONSUMER_CONTRACT OR AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED)
        if(AROS_NATIVE_BUILD_CONTRACT OR AROS_NATIVE_BUILD_CONTRACT_VALIDATED)
            _aros_native_consumer_fail("build and consumer selections are mutually exclusive")
        endif()
        _aros_native_consumer_current(_source _path _digest)
        set(${out_validated} TRUE PARENT_SCOPE)
    elseif(AROS_NATIVE_BUILD_CONTRACT_VALIDATED)
        _aros_native_require_current_validation(_source _path _digest)
        file(REAL_PATH "${AROS_SOURCE_DIR}" _current_source)
        if(NOT _current_source STREQUAL _source OR
           NOT AROS_NATIVE_BUILD_CONTRACT STREQUAL _path OR
           NOT AROS_NATIVE_BUILD_CONTRACT_SHA256 STREQUAL _digest)
            _aros_native_contract_fail("source selection" "configuration changed after validation")
        endif()
        file(SHA256 "${_path}" _current_digest)
        if(NOT _current_digest STREQUAL _digest)
            _aros_native_contract_fail("source selection" "contract changed after validation")
        endif()
        set(${out_validated} TRUE PARENT_SCOPE)
    else()
        set(${out_validated} FALSE PARENT_SCOPE)
    endif()
endfunction()

function(aros_lock_native_consumer_contract_identity)
    _aros_native_consumer_current(_source _path _digest)
    set(_any FALSE)
    set(_all TRUE)
    foreach(_key IN ITEMS SOURCE_DIR PATH SHA256)
        if(DEFINED AROS_NATIVE_CONSUMER_CONTRACT_LOCKED_${_key})
            set(_any TRUE)
        else()
            set(_all FALSE)
        endif()
    endforeach()
    if(_any AND (NOT _all OR
       NOT AROS_NATIVE_CONSUMER_CONTRACT_LOCKED_SOURCE_DIR STREQUAL _source OR
       NOT AROS_NATIVE_CONSUMER_CONTRACT_LOCKED_PATH STREQUAL _path OR
       NOT AROS_NATIVE_CONSUMER_CONTRACT_LOCKED_SHA256 STREQUAL _digest))
        _aros_native_consumer_fail("build tree is already bound to another consumer contract; use a separate tree")
    endif()
    set(AROS_NATIVE_CONSUMER_CONTRACT_LOCKED_SOURCE_DIR "${_source}" CACHE INTERNAL
        "Source root bound to this consumer build tree" FORCE)
    set(AROS_NATIVE_CONSUMER_CONTRACT_LOCKED_PATH "${_path}" CACHE INTERNAL
        "Consumer contract bound to this build tree" FORCE)
    set(AROS_NATIVE_CONSUMER_CONTRACT_LOCKED_SHA256 "${_digest}" CACHE INTERNAL
        "Consumer contract digest bound to this build tree" FORCE)
endfunction()

function(aros_validate_native_consumer_compiler_target)
    _aros_native_consumer_current(_source _path _digest)
    foreach(_selector IN ITEMS march:ISA mabi:ABI mcmodel:CODE_MODEL)
        string(REPLACE ":" ";" _parts "${_selector}")
        list(GET _parts 0 _flag)
        list(GET _parts 1 _key)
        get_property(_value GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_${_key})
        set(_count 0)
        foreach(_option IN LISTS AROS_GNU_TARGET_COMPILE_OPTIONS)
            if(_option MATCHES "^-${_flag}")
                math(EXPR _count "${_count} + 1")
                if(NOT _option STREQUAL "-${_flag}=${_value}")
                    _aros_native_consumer_fail("GNU -${_flag} differs from the validated ABI")
                endif()
            endif()
        endforeach()
        if(NOT _count EQUAL 1)
            _aros_native_consumer_fail("requires exactly one GNU -${_flag} option")
        endif()
    endforeach()
endfunction()
