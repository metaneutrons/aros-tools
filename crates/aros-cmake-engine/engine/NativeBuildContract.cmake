# Fail-closed CMake boundary for source-owned native build contracts.
#
# This module validates and exports contract facts. It does not create core,
# package, media, or bootloader build rules; consumers must still implement
# those rules from the validated AROS_NATIVE_BUILD_* values.

include_guard(GLOBAL)

# Both inventory passes and the replay record must use this exact source
# selection. Only nonempty validated identity arguments are expanded as a
# CMake list; the caller's ordinary empty ABI selectors remain quoted.
function(aros_native_transpiler_arguments out_var)
    if(AROS_NATIVE_CONSUMER_CONTRACT)
        if(AROS_NATIVE_BUILD_CONTRACT)
            message(FATAL_ERROR "Native build and consumer selections are mutually exclusive")
        endif()
        _aros_native_consumer_current(_source _path _digest)
        set(${out_var}
            --native-consumer-profile "${AROS_NATIVE_CONSUMER_PROFILE}"
            --native-consumer-contract-sha256 "${_digest}"
            PARENT_SCOPE)
        return()
    endif()
    if(NOT AROS_NATIVE_BUILD_CONTRACT)
        set(${out_var} "" PARENT_SCOPE)
        return()
    endif()
    if(NOT AROS_NATIVE_BUILD_CONTRACT_VALIDATED OR
       NOT AROS_NATIVE_BUILD_PROFILE OR NOT AROS_NATIVE_BUILD_CONTRACT_SHA256)
        message(FATAL_ERROR "Native transpiler arguments require validated source identity")
    endif()
    set(${out_var}
        --native-profile "${AROS_NATIVE_BUILD_PROFILE}"
        --native-contract-sha256 "${AROS_NATIVE_BUILD_CONTRACT_SHA256}"
        PARENT_SCOPE)
endfunction()

function(_aros_native_contract_fail label reason)
    message(FATAL_ERROR "Native build contract ${label}: ${reason}")
endfunction()

function(_aros_native_require_object_members json label expected_fields)
    string(JSON _type ERROR_VARIABLE _json_error TYPE "${json}")
    if(NOT _json_error STREQUAL "NOTFOUND" OR NOT _type STREQUAL "OBJECT")
        _aros_native_contract_fail("${label}" "must be a JSON object")
    endif()

    string(JSON _member_count LENGTH "${json}")
    set(_expected ${expected_fields})
    list(LENGTH _expected _expected_count)

    set(_seen "")
    if(_member_count GREATER 0)
        math(EXPR _last_member "${_member_count} - 1")
        foreach(_index RANGE 0 ${_last_member})
            string(JSON _member MEMBER "${json}" ${_index})
            list(FIND _expected "${_member}" _expected_index)
            if(_expected_index EQUAL -1)
                _aros_native_contract_fail("${label}" "has unknown field '${_member}'")
            endif()
            list(FIND _seen "${_member}" _seen_index)
            if(NOT _seen_index EQUAL -1)
                _aros_native_contract_fail("${label}" "repeats field '${_member}'")
            endif()
            list(APPEND _seen "${_member}")
        endforeach()
    endif()

    foreach(_member IN LISTS _expected)
        list(FIND _seen "${_member}" _seen_index)
        if(_seen_index EQUAL -1)
            _aros_native_contract_fail("${label}" "omits required field '${_member}'")
        endif()
    endforeach()
    if(NOT _member_count EQUAL _expected_count)
        _aros_native_contract_fail("${label}"
            "has ${_member_count} fields; expected exactly ${_expected_count}")
    endif()
endfunction()

function(_aros_native_require_type json label member expected_type)
    string(JSON _actual_type ERROR_VARIABLE _json_error TYPE "${json}" "${member}")
    if(NOT _json_error STREQUAL "NOTFOUND" OR
       NOT _actual_type STREQUAL "${expected_type}")
        _aros_native_contract_fail("${label}.${member}" "must be ${expected_type}")
    endif()
endfunction()

function(_aros_native_get_string out_value json label member)
    _aros_native_require_type("${json}" "${label}" "${member}" STRING)
    string(JSON _value GET "${json}" "${member}")
    set(${out_value} "${_value}" PARENT_SCOPE)
endfunction()

function(_aros_native_validate_token value label)
    if("${value}" STREQUAL "" OR
       NOT "${value}" MATCHES "^[A-Za-z0-9_.+-]+$")
        _aros_native_contract_fail("${label}" "must be a portable non-empty code token")
    endif()
endfunction()

function(_aros_native_validate_sha256 value label)
    string(LENGTH "${value}" _length)
    if(NOT _length EQUAL 64 OR NOT "${value}" MATCHES "^[0-9a-f]+$")
        _aros_native_contract_fail("${label}" "must be a lowercase SHA-256 digest")
    endif()
endfunction()

function(_aros_native_validate_source_path out_path value label)
    string(LENGTH "${value}" _length)
    if(_length LESS 1 OR _length GREATER 4096 OR
       NOT "${value}" MATCHES "^[A-Za-z0-9_.+-]+(/[A-Za-z0-9_.+-]+)*$")
        _aros_native_contract_fail("${label}"
            "must be a portable, canonical source-relative path")
    endif()

    string(REPLACE "/" ";" _segments "${value}")
    foreach(_segment IN LISTS _segments)
        if(_segment STREQUAL "." OR _segment STREQUAL "..")
            _aros_native_contract_fail("${label}"
                "must not contain current-directory or parent-directory segments")
        endif()
    endforeach()
    set(${out_path} "${value}" PARENT_SCOPE)
endfunction()

function(_aros_native_resolve_source_file out_real source_root relative_path label)
    _aros_native_validate_source_path(_checked_path "${relative_path}" "${label}")
    set(_candidate "${source_root}/${_checked_path}")
    if(NOT EXISTS "${_candidate}" OR IS_DIRECTORY "${_candidate}")
        _aros_native_contract_fail("${label}" "does not resolve to a source file")
    endif()

    file(REAL_PATH "${_candidate}" _resolved)
    cmake_path(IS_PREFIX source_root "${_resolved}" NORMALIZE _inside_source)
    if(NOT _inside_source)
        _aros_native_contract_fail("${label}" "resolves outside AROS_SOURCE_DIR")
    endif()
    if(IS_DIRECTORY "${_resolved}")
        _aros_native_contract_fail("${label}" "resolves to a directory, not a file")
    endif()
    set(${out_real} "${_resolved}" PARENT_SCOPE)
endfunction()

function(_aros_native_require_non_symlink_source_file source_root relative_path label)
    _aros_native_validate_source_path(_checked_path "${relative_path}" "${label}")
    string(REPLACE "/" ";" _segments "${_checked_path}")
    set(_probe "${source_root}")
    foreach(_segment IN LISTS _segments)
        set(_probe "${_probe}/${_segment}")
        if(IS_SYMLINK "${_probe}")
            _aros_native_contract_fail("${label}" "path crosses a symlink")
        endif()
        if(NOT EXISTS "${_probe}")
            _aros_native_contract_fail("${label}" "does not resolve to a source file")
        endif()
    endforeach()
    _aros_native_resolve_source_file(_resolved "${source_root}" "${_checked_path}" "${label}")
endfunction()

function(_aros_native_get_make_include_bindings out_json contract_json source_root input_paths)
    set(_bindings_json "{}")
    string(JSON _bindings_type ERROR_VARIABLE _bindings_error
        TYPE "${contract_json}" make_include_bindings)
    if(_bindings_error STREQUAL "NOTFOUND")
        if(NOT _bindings_type STREQUAL "OBJECT")
            _aros_native_contract_fail("make_include_bindings" "must be an object")
        endif()
        string(JSON _bindings_json GET "${contract_json}" make_include_bindings)
        string(JSON _binding_count LENGTH "${_bindings_json}")
        if(_binding_count GREATER 16)
            _aros_native_contract_fail("make_include_bindings" "exceeds 16 entries")
        endif()

        set(_seen_keys "")
        set(_seen_replacements "")
        if(_binding_count GREATER 0)
            math(EXPR _last_binding "${_binding_count} - 1")
            foreach(_index RANGE 0 ${_last_binding})
                string(JSON _include MEMBER "${_bindings_json}" ${_index})
                _aros_native_validate_source_path(_include "${_include}"
                    "make_include_bindings key")
                list(FIND _seen_keys "${_include}" _duplicate_key)
                if(NOT _duplicate_key EQUAL -1)
                    _aros_native_contract_fail("make_include_bindings"
                        "contains duplicate key '${_include}'")
                endif()
                list(APPEND _seen_keys "${_include}")

                string(JSON _replacement_type ERROR_VARIABLE _replacement_error
                    TYPE "${_bindings_json}" "${_include}")
                if(NOT _replacement_error STREQUAL "NOTFOUND" OR
                   NOT _replacement_type STREQUAL "STRING")
                    _aros_native_contract_fail("make_include_bindings.${_include}"
                        "must be a string replacement path")
                endif()
                string(JSON _replacement GET "${_bindings_json}" "${_include}")
                _aros_native_validate_source_path(_replacement "${_replacement}"
                    "make_include_bindings.${_include}")
                if(NOT _replacement MATCHES "\\.mk$")
                    _aros_native_contract_fail("make_include_bindings.${_include}"
                        "replacement must name a .mk file")
                endif()

                string(TOLOWER "${_replacement}" _replacement_key)
                list(FIND _seen_replacements "${_replacement_key}" _duplicate_replacement)
                if(NOT _duplicate_replacement EQUAL -1)
                    _aros_native_contract_fail("make_include_bindings"
                        "contains duplicate replacement '${_replacement}'")
                endif()
                list(APPEND _seen_replacements "${_replacement_key}")

                list(FIND input_paths "${_include}" _include_input_index)
                if(_include_input_index EQUAL -1)
                    _aros_native_contract_fail("make_include_bindings key"
                        "path '${_include}' is not declared in inputs")
                endif()
                list(FIND input_paths "${_replacement}" _replacement_input_index)
                if(_replacement_input_index EQUAL -1)
                    _aros_native_contract_fail("make_include_bindings replacement"
                        "path '${_replacement}' is not declared in inputs")
                endif()
                _aros_native_require_non_symlink_source_file("${source_root}"
                    "${_include}" "make_include_bindings key '${_include}'")
                _aros_native_require_non_symlink_source_file("${source_root}"
                    "${_replacement}" "make_include_bindings replacement '${_replacement}'")
            endforeach()
        endif()
    endif()
    set(${out_json} "${_bindings_json}" PARENT_SCOPE)
endfunction()

function(_aros_native_get_source_path out_value json label member input_paths)
    _aros_native_get_string(_value "${json}" "${label}" "${member}")
    _aros_native_validate_source_path(_checked_path "${_value}" "${label}.${member}")
    list(FIND input_paths "${_checked_path}" _input_index)
    if(_input_index EQUAL -1)
        _aros_native_contract_fail("${label}.${member}"
            "path '${_checked_path}' is not declared in inputs")
    endif()
    set(${out_value} "${_checked_path}" PARENT_SCOPE)
endfunction()

function(_aros_native_get_token out_value json label member)
    _aros_native_get_string(_value "${json}" "${label}" "${member}")
    _aros_native_validate_token("${_value}" "${label}.${member}")
    set(${out_value} "${_value}" PARENT_SCOPE)
endfunction()

function(_aros_native_validate_positive_safe_unsigned out_value value label)
    string(LENGTH "${value}" _length)
    if(NOT "${value}" MATCHES "^[1-9][0-9]*$" OR _length GREATER 19 OR
       (_length EQUAL 19 AND "${value}" STRGREATER "9223372036854775807"))
        _aros_native_contract_fail("${label}"
            "must be a positive safe unsigned integer no greater than 9223372036854775807")
    endif()
    set(${out_value} "${value}" PARENT_SCOPE)
endfunction()

function(_aros_native_get_string_array out_values json label member)
    _aros_native_require_type("${json}" "${label}" "${member}" ARRAY)
    string(JSON _length LENGTH "${json}" "${member}")
    set(_values "")
    foreach(_index RANGE 0 ${_length})
        if(_index GREATER_EQUAL _length)
            break()
        endif()
        string(JSON _item_type ERROR_VARIABLE _item_error TYPE
            "${json}" "${member}" ${_index})
        if(NOT _item_error STREQUAL "NOTFOUND" OR
           NOT _item_type STREQUAL "STRING")
            _aros_native_contract_fail("${label}.${member}[${_index}]"
                "must be a string")
        endif()
        string(JSON _item GET "${json}" "${member}" ${_index})
        _aros_native_validate_token("${_item}" "${label}.${member}[${_index}]")
        list(FIND _values "${_item}" _duplicate_index)
        if(NOT _duplicate_index EQUAL -1)
            _aros_native_contract_fail("${label}.${member}"
                "contains duplicate token '${_item}'")
        endif()
        list(APPEND _values "${_item}")
    endforeach()
    set(${out_values} "${_values}" PARENT_SCOPE)
endfunction()

function(_aros_native_json_boolean out_value json label member)
    _aros_native_require_type("${json}" "${label}" "${member}" BOOLEAN)
    string(JSON _value GET "${json}" "${member}")
    if(_value MATCHES "^(ON|TRUE|1)$")
        set(_normalized ON)
    elseif(_value MATCHES "^(OFF|FALSE|0)$")
        set(_normalized OFF)
    else()
        _aros_native_contract_fail("${label}.${member}" "is not a JSON boolean")
    endif()
    set(${out_value} "${_normalized}" PARENT_SCOPE)
endfunction()

function(_aros_native_selector_boolean out_value value label)
    string(TOUPPER "${value}" _upper_value)
    if(_upper_value MATCHES "^(ON|TRUE|YES|Y|1)$")
        set(_normalized ON)
    elseif(_upper_value MATCHES "^(OFF|FALSE|NO|N|0)$")
        set(_normalized OFF)
    else()
        _aros_native_contract_fail("${label}" "must be an explicit ON/OFF boolean")
    endif()
    set(${out_value} "${_normalized}" PARENT_SCOPE)
endfunction()

function(_aros_native_require_current_validation out_source out_path out_sha)
    get_property(_validated GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED)
    if(NOT _validated)
        _aros_native_contract_fail("API" "source validation must run first")
    endif()
    get_property(_source GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_SOURCE)
    get_property(_path GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_PATH)
    get_property(_sha GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_SHA256)
    set(${out_source} "${_source}" PARENT_SCOPE)
    set(${out_path} "${_path}" PARENT_SCOPE)
    set(${out_sha} "${_sha}" PARENT_SCOPE)
endfunction()

function(aros_validate_native_build_contract)
    foreach(_required IN ITEMS
            AROS_NATIVE_BUILD_CONTRACT AROS_NATIVE_BUILD_CONTRACT_SHA256
            AROS_SOURCE_DIR AROS_TARGET_PROFILE AROS_TARGET_CPU AROS_TARGET_TRIPLE
            AROS_TOOLCHAIN GCC_CONFIG_FLOAT_ABI AROS_ABI_FLAVOUR
            AROS_ABI_PLATFORM_SMP AROS_ENABLE_MMU)
        if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
            _aros_native_contract_fail("configuration" "${_required} must be provided")
        endif()
    endforeach()

    _aros_native_validate_sha256("${AROS_NATIVE_BUILD_CONTRACT_SHA256}"
        "AROS_NATIVE_BUILD_CONTRACT_SHA256")
    if(NOT AROS_TOOLCHAIN STREQUAL "gnu")
        _aros_native_contract_fail("ABI" "AROS_TOOLCHAIN must be gnu")
    endif()

    foreach(_selector IN ITEMS AROS_TARGET_PROFILE AROS_TARGET_CPU
            AROS_TARGET_TRIPLE GCC_CONFIG_FLOAT_ABI AROS_ABI_FLAVOUR)
        _aros_native_validate_token("${${_selector}}" "${_selector}")
    endforeach()

    _aros_native_selector_boolean(_selected_platform_smp "${AROS_ABI_PLATFORM_SMP}"
        AROS_ABI_PLATFORM_SMP)
    _aros_native_selector_boolean(_selected_use_mmu "${AROS_ENABLE_MMU}"
        AROS_ENABLE_MMU)

    if(NOT IS_DIRECTORY "${AROS_SOURCE_DIR}")
        _aros_native_contract_fail("AROS_SOURCE_DIR" "is not an existing directory")
    endif()
    file(REAL_PATH "${AROS_SOURCE_DIR}" _source_root)

    if(NOT IS_ABSOLUTE "${AROS_NATIVE_BUILD_CONTRACT}")
        _aros_native_contract_fail("path" "AROS_NATIVE_BUILD_CONTRACT must be absolute")
    endif()
    if(NOT EXISTS "${AROS_NATIVE_BUILD_CONTRACT}" OR
       IS_DIRECTORY "${AROS_NATIVE_BUILD_CONTRACT}")
        _aros_native_contract_fail("path" "contract must be an existing regular file")
    endif()
    file(REAL_PATH "${AROS_NATIVE_BUILD_CONTRACT}" _contract_path)
    if(NOT "${AROS_NATIVE_BUILD_CONTRACT}" STREQUAL "${_contract_path}")
        _aros_native_contract_fail("path" "contract path must already be canonical")
    endif()
    cmake_path(IS_PREFIX _source_root "${_contract_path}" NORMALIZE _contract_inside)
    if(NOT _contract_inside)
        _aros_native_contract_fail("path" "contract resolves outside AROS_SOURCE_DIR")
    endif()

    file(SIZE "${_contract_path}" _contract_size)
    if(_contract_size GREATER 1048576)
        _aros_native_contract_fail("size" "contract exceeds the 1048576-byte limit")
    endif()
    # Read at most one byte beyond the limit as a second guard against a file
    # that grows between the size check and the read.
    file(READ "${_contract_path}" _contract_json LIMIT 1048577)
    string(LENGTH "${_contract_json}" _contract_read_length)
    if(_contract_read_length GREATER 1048576)
        _aros_native_contract_fail("size" "contract exceeds the 1048576-byte limit")
    endif()
    string(SHA256 _actual_contract_sha "${_contract_json}")
    if(NOT _actual_contract_sha STREQUAL AROS_NATIVE_BUILD_CONTRACT_SHA256)
        _aros_native_contract_fail("digest"
            "contract bytes do not match AROS_NATIVE_BUILD_CONTRACT_SHA256")
    endif()

    set(_top_fields
        "schema_version;profile;board;source_baseline;qualification;inputs;abi;core;package;media")
    string(JSON _make_variables_type ERROR_VARIABLE _make_variables_error TYPE "${_contract_json}" make_variables)
    set(_make_variables_json "{}")
    if(_make_variables_error STREQUAL "NOTFOUND")
        list(APPEND _top_fields make_variables)
        if(NOT _make_variables_type STREQUAL "OBJECT")
            _aros_native_contract_fail("make_variables" "must be an object")
        endif()
        string(JSON _make_variables_json GET "${_contract_json}" make_variables)
        string(JSON _make_variable_count LENGTH "${_make_variables_json}")
        if(_make_variable_count GREATER 256)
            _aros_native_contract_fail("make_variables" "exceeds 256 entries")
        endif()
        set(_reserved_make_variables
            AROS_TARGET_CPU CPU AROS_TARGET_ARCH ARCH AROS_TARGET_PLATFORM
            AROS_TARGET_FAMILY FAMILY AROS_TARGET_VARIANT AROS_TOOLCHAIN
            AROS_TARGET_CPU32 USE_MMU GCC_CONFIG_FLOAT_ABI OPT_MESAGL
            TARGET_LLVM_VER TARGET_LLVM_RUNTIMES_STYLE TARGET_RUST TARGET_RUST_VER)
        if(_make_variable_count GREATER 0)
            math(EXPR _last_make_variable "${_make_variable_count} - 1")
            foreach(_index RANGE 0 ${_last_make_variable})
                string(JSON _name MEMBER "${_make_variables_json}" ${_index})
                string(LENGTH "${_name}" _name_length)
                if(_name_length GREATER 64 OR NOT _name MATCHES "^[A-Z_][A-Z_0-9]*$" OR
                   _name IN_LIST _reserved_make_variables)
                    _aros_native_contract_fail("make_variables" "unsafe name or target selector shadow: '${_name}'")
                endif()
                _aros_native_get_string(_value "${_make_variables_json}" make_variables "${_name}")
                string(LENGTH "${_value}" _value_length)
                if(_value_length GREATER 128 OR NOT _value MATCHES "^[A-Za-z0-9_.+-]*$")
                    _aros_native_contract_fail("make_variables.${_name}" "must be a literal scalar, possibly empty")
                endif()
            endforeach()
        endif()
    endif()
    string(JSON _make_include_bindings_type ERROR_VARIABLE _make_include_bindings_error
        TYPE "${_contract_json}" make_include_bindings)
    if(_make_include_bindings_error STREQUAL "NOTFOUND")
        list(APPEND _top_fields make_include_bindings)
        if(NOT _make_include_bindings_type STREQUAL "OBJECT")
            _aros_native_contract_fail("make_include_bindings" "must be an object")
        endif()
    endif()
    string(JSON _optional_meta_type ERROR_VARIABLE _optional_meta_error
        TYPE "${_contract_json}" optional_meta_dependencies)
    if(_optional_meta_error STREQUAL "NOTFOUND")
        list(APPEND _top_fields optional_meta_dependencies)
        if(NOT _optional_meta_type STREQUAL "ARRAY")
            _aros_native_contract_fail("optional_meta_dependencies" "must be an array")
        endif()
    endif()
    # These exports are not executable CMake commands. The mandatory Rust
    # transpiler pass independently checks their closed metadata and exact
    # source recipes before publishing any corresponding generator target.
    string(JSON _host_files_type ERROR_VARIABLE _host_files_error
        TYPE "${_contract_json}" host_file_generators)
    if(_host_files_error STREQUAL "NOTFOUND")
        list(APPEND _top_fields host_file_generators)
        if(NOT _host_files_type STREQUAL "ARRAY")
            _aros_native_contract_fail("host_file_generators" "must be an array")
        endif()
        string(JSON _host_files_count LENGTH "${_contract_json}" host_file_generators)
        if(_host_files_count GREATER 16)
            _aros_native_contract_fail("host_file_generators" "exceeds 16 entries")
        endif()
    endif()
    # Consumed by the Rust transpiler, which verifies their content against the
    # sealed sources; the engine only checks that each is present as the type
    # the contract schema gives it.
    foreach(_rust_field IN ITEMS host_make_variables:OBJECT generated_make_templates:OBJECT
            metamake_projection:STRING kernel_compiler_role:STRING)
        string(REPLACE ":" ";" _rust_field_parts "${_rust_field}")
        list(GET _rust_field_parts 0 _rust_field_name)
        list(GET _rust_field_parts 1 _rust_field_type)
        string(JSON _rust_field_actual ERROR_VARIABLE _rust_field_error
            TYPE "${_contract_json}" "${_rust_field_name}")
        if(_rust_field_error STREQUAL "NOTFOUND")
            list(APPEND _top_fields "${_rust_field_name}")
            if(NOT _rust_field_actual STREQUAL _rust_field_type)
                _aros_native_contract_fail("${_rust_field_name}"
                    "must be a JSON ${_rust_field_type}")
            endif()
        endif()
    endforeach()
    # configure writes ENABLE_EXECSMP into the Make templates and into
    # aros/config.h alike. The contract carries it once, as the substitution of
    # @ENABLE_EXECSMP@, so aros/config.h follows what the Make side reads.
    set(_exec_smp OFF)
    string(JSON _templates_json ERROR_VARIABLE _templates_error
        GET "${_contract_json}" generated_make_templates)
    if(_templates_error STREQUAL "NOTFOUND")
        string(JSON _template_count LENGTH "${_templates_json}")
        if(_template_count GREATER 0)
            math(EXPR _last_template "${_template_count} - 1")
            foreach(_template_index RANGE 0 ${_last_template})
                string(JSON _template_path MEMBER "${_templates_json}" ${_template_index})
                string(JSON _smp_substitution ERROR_VARIABLE _smp_error
                    GET "${_templates_json}" "${_template_path}" substitutions "@ENABLE_EXECSMP@")
                if(NOT _smp_error STREQUAL "NOTFOUND")
                    continue()
                endif()
                if(_smp_substitution STREQUAL "#define __AROSEXEC_SMP__")
                    set(_template_smp ON)
                elseif(_smp_substitution STREQUAL "")
                    set(_template_smp OFF)
                else()
                    _aros_native_contract_fail("generated_make_templates"
                        "@ENABLE_EXECSMP@ must be empty or \"#define __AROSEXEC_SMP__\"")
                endif()
                if(DEFINED _template_seen AND NOT _template_smp STREQUAL _exec_smp)
                    _aros_native_contract_fail("generated_make_templates"
                        "templates disagree on @ENABLE_EXECSMP@")
                endif()
                set(_template_seen TRUE)
                set(_exec_smp "${_template_smp}")
            endforeach()
        endif()
    endif()
    string(JSON _kernel_role ERROR_VARIABLE _kernel_role_error
        GET "${_contract_json}" kernel_compiler_role)
    if(_kernel_role_error STREQUAL "NOTFOUND" AND NOT _kernel_role STREQUAL "target")
        _aros_native_contract_fail("kernel_compiler_role"
            "the only admitted value is \"target\"")
    endif()
    _aros_native_require_object_members("${_contract_json}" "root object" "${_top_fields}")

    _aros_native_require_type("${_contract_json}" "contract" schema_version NUMBER)
    string(JSON _schema_version GET "${_contract_json}" schema_version)
    if(NOT _schema_version MATCHES "^[0-9]+$" OR NOT _schema_version STREQUAL "1")
        _aros_native_contract_fail("schema_version" "must be the integer 1")
    endif()
    _aros_native_get_token(_profile "${_contract_json}" contract profile)
    _aros_native_get_token(_board "${_contract_json}" contract board)
    _aros_native_get_string(_source_baseline "${_contract_json}" contract source_baseline)
    string(LENGTH "${_source_baseline}" _source_baseline_length)
    if(NOT _source_baseline_length EQUAL 40 OR
       NOT _source_baseline MATCHES "^[A-Fa-f0-9]+$")
        _aros_native_contract_fail("source_baseline"
            "must be exactly 40 hexadecimal characters")
    endif()
    _aros_native_get_string(_qualification "${_contract_json}" contract qualification)
    if(NOT _qualification STREQUAL "experimental-unqualified")
        _aros_native_contract_fail("qualification"
            "must be experimental-unqualified")
    endif()
    if(NOT _profile STREQUAL AROS_TARGET_PROFILE)
        _aros_native_contract_fail("profile"
            "'${_profile}' does not match selected target '${AROS_TARGET_PROFILE}'")
    endif()

    _aros_native_require_type("${_contract_json}" "contract" inputs ARRAY)
    string(JSON _input_count LENGTH "${_contract_json}" inputs)
    if(_input_count LESS 1 OR _input_count GREATER 128)
        _aros_native_contract_fail("inputs" "must contain between 1 and 128 entries")
    endif()

    set(_input_paths "")
    set(_resolved_inputs "")
    set(_source_files "${_contract_path}")
    math(EXPR _last_input "${_input_count} - 1")
    foreach(_index RANGE 0 ${_last_input})
        string(JSON _input_json ERROR_VARIABLE _input_error GET
            "${_contract_json}" inputs ${_index})
        if(NOT _input_error STREQUAL "NOTFOUND")
            _aros_native_contract_fail("inputs[${_index}]" "is not a valid JSON value")
        endif()
        _aros_native_require_object_members("${_input_json}" "inputs[${_index}]"
            "path;sha256")
        _aros_native_get_string(_input_path "${_input_json}" "inputs[${_index}]" path)
        _aros_native_validate_source_path(_input_path "${_input_path}"
            "inputs[${_index}].path")
        string(TOLOWER "${_input_path}" _input_path_key)
        list(FIND _input_paths "${_input_path_key}" _duplicate_path)
        if(NOT _duplicate_path EQUAL -1)
            _aros_native_contract_fail("inputs[${_index}].path"
                "duplicates another declared path")
        endif()
        _aros_native_get_string(_input_sha "${_input_json}" "inputs[${_index}]" sha256)
        _aros_native_validate_sha256("${_input_sha}" "inputs[${_index}].sha256")

        _aros_native_resolve_source_file(_resolved_input "${_source_root}"
            "${_input_path}" "inputs[${_index}].path")
        string(TOLOWER "${_resolved_input}" _resolved_input_key)
        list(FIND _resolved_inputs "${_resolved_input_key}" _duplicate_resolved)
        if(NOT _duplicate_resolved EQUAL -1)
            _aros_native_contract_fail("inputs[${_index}].path"
                "resolves to a file already declared by another input")
        endif()
        file(SHA256 "${_resolved_input}" _measured_sha)
        if(NOT _measured_sha STREQUAL _input_sha)
            _aros_native_contract_fail("inputs[${_index}].path"
                "SHA-256 differs from the declared digest")
        endif()

        list(APPEND _input_paths "${_input_path}")
        list(APPEND _input_paths "${_input_path_key}")
        list(APPEND _resolved_inputs "${_resolved_input_key}")
        list(APPEND _source_files "${_resolved_input}")
        set(_input_${_index}_path "${_input_path}")
        set(_input_${_index}_sha "${_input_sha}")
    endforeach()
    if(NOT CMAKE_SCRIPT_MODE_FILE)
        # Ninja must re-enter validation when any source-owned fact changes,
        # including board/partition files not otherwise read by the graph.
        set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
            ${_source_files})
    endif()
    # Keep path lookup separate from its lower-case duplicate-detection keys.
    set(_declared_input_paths "")
    foreach(_index RANGE 0 ${_last_input})
        list(APPEND _declared_input_paths "${_input_${_index}_path}")
    endforeach()
    _aros_native_get_make_include_bindings(_make_include_bindings_json
        "${_contract_json}" "${_source_root}" "${_declared_input_paths}")

    if(_optional_meta_error STREQUAL "NOTFOUND")
        string(JSON _optional_meta_count LENGTH "${_contract_json}" optional_meta_dependencies)
        if(_optional_meta_count GREATER 64)
            _aros_native_contract_fail("optional_meta_dependencies" "exceeds 64 entries")
        endif()
        set(_optional_meta_seen "")
        if(_optional_meta_count GREATER 0)
            math(EXPR _optional_meta_last "${_optional_meta_count} - 1")
            foreach(_index RANGE 0 ${_optional_meta_last})
                string(JSON _edge GET "${_contract_json}" optional_meta_dependencies ${_index})
                string(JSON _absence_type ERROR_VARIABLE _absence_error TYPE "${_edge}" absence)
                set(_edge_absence "selector")
                set(_edge_fields "recipe;target;dependency")
                if(_absence_error STREQUAL "NOTFOUND")
                    _aros_native_get_string(_edge_absence "${_edge}" optional_meta_dependencies absence)
                    list(APPEND _edge_fields absence)
                endif()
                _aros_native_require_object_members("${_edge}" "optional_meta_dependencies[${_index}]"
                    "${_edge_fields}")
                if(NOT _edge_absence STREQUAL "selector" AND NOT _edge_absence STREQUAL "disabled-owner")
                    _aros_native_contract_fail("optional_meta_dependencies.absence" "unknown absence proof")
                endif()
                foreach(_field IN ITEMS recipe target dependency)
                    _aros_native_get_string(_edge_${_field} "${_edge}" optional_meta_dependencies "${_field}")
                endforeach()
                _aros_native_validate_source_path(_edge_recipe_path "${_edge_recipe}" "optional MetaMake recipe")
                if(NOT _edge_recipe IN_LIST _declared_input_paths OR
                   NOT _edge_recipe MATCHES "(^|/)[A-Za-z0-9_.+-]+\\.src$|(^|/)mmakefile$")
                    _aros_native_contract_fail("optional_meta_dependencies.recipe"
                        "must name an inventoried MetaMake recipe")
                endif()
                if(NOT _edge_target MATCHES "^[A-Za-z0-9_.+-]+$")
                    _aros_native_contract_fail("optional_meta_dependencies.target" "must be a literal target")
                endif()
                set(_literal_dependency "${_edge_dependency}")
                foreach(_selector IN ITEMS CPU PLATFORM LEGACY_PLATFORM FAMILY VARIANT CPU32)
                    string(REPLACE "\${AROS_TARGET_${_selector}}" "selector"
                        _literal_dependency "${_literal_dependency}")
                endforeach()
                if(_edge_absence STREQUAL "disabled-owner")
                    if(NOT _edge_dependency MATCHES "^[A-Za-z0-9_.+-]+$")
                        _aros_native_contract_fail("optional_meta_dependencies.dependency"
                            "disabled owner must be a literal target")
                    endif()
                elseif(_literal_dependency STREQUAL _edge_dependency OR
                       NOT _literal_dependency MATCHES "^[A-Za-z0-9_.+-]+$")
                    _aros_native_contract_fail("optional_meta_dependencies.dependency"
                        "must be a safe selector-derived target template")
                endif()
                set(_edge_key "${_edge_recipe}|${_edge_target}|${_edge_dependency}")
                if(_edge_key IN_LIST _optional_meta_seen)
                    _aros_native_contract_fail("optional_meta_dependencies" "repeats an edge")
                endif()
                list(APPEND _optional_meta_seen "${_edge_key}")
            endforeach()
        endif()
    endif()

    string(JSON _abi_json GET "${_contract_json}" abi)
    _aros_native_require_object_members("${_abi_json}" abi
        "source_cpu;target_triple;isa;abi;code_model;flavour;platform_smp;use_mmu")
    foreach(_field IN ITEMS source_cpu target_triple isa abi code_model flavour)
        _aros_native_get_token(_abi_${_field} "${_abi_json}" abi "${_field}")
    endforeach()
    _aros_native_json_boolean(_abi_platform_smp "${_abi_json}" abi platform_smp)
    _aros_native_json_boolean(_abi_use_mmu "${_abi_json}" abi use_mmu)
    if(NOT _abi_source_cpu STREQUAL AROS_TARGET_CPU)
        _aros_native_contract_fail("abi.source_cpu"
            "'${_abi_source_cpu}' does not match selected CPU '${AROS_TARGET_CPU}'")
    endif()
    set(_expected_triple "${_abi_source_cpu}-aros")
    if(NOT _abi_target_triple STREQUAL _expected_triple OR
       NOT _abi_target_triple STREQUAL AROS_TARGET_TRIPLE)
        _aros_native_contract_fail("abi.target_triple"
            "does not match selected target triple '${AROS_TARGET_TRIPLE}'")
    endif()
    if(NOT _abi_code_model MATCHES "^(medlow|medany)$")
        _aros_native_contract_fail("abi.code_model" "must be medlow or medany")
    endif()
    if(NOT _abi_flavour MATCHES "^(native|standalone|emulation)$")
        _aros_native_contract_fail("abi.flavour" "is not a supported ABI flavour")
    endif()
    if(NOT _abi_abi STREQUAL GCC_CONFIG_FLOAT_ABI)
        _aros_native_contract_fail("abi.abi"
            "'${_abi_abi}' does not match GCC_CONFIG_FLOAT_ABI '${GCC_CONFIG_FLOAT_ABI}'")
    endif()
    if(NOT _abi_flavour STREQUAL AROS_ABI_FLAVOUR)
        _aros_native_contract_fail("abi.flavour"
            "does not match AROS_ABI_FLAVOUR '${AROS_ABI_FLAVOUR}'")
    endif()
    if(NOT _abi_platform_smp STREQUAL _selected_platform_smp)
        _aros_native_contract_fail("abi.platform_smp"
            "does not match AROS_ABI_PLATFORM_SMP '${AROS_ABI_PLATFORM_SMP}'")
    endif()
    if(NOT _abi_use_mmu STREQUAL _selected_use_mmu)
        _aros_native_contract_fail("abi.use_mmu"
            "does not match AROS_ENABLE_MMU '${AROS_ENABLE_MMU}'")
    endif()

    string(JSON _core_json GET "${_contract_json}" core)
    _aros_native_require_object_members("${_core_json}" core
        "recipe;linker_script;resources;libraries;devices;link_libraries;compiler_runtime_role;residency_check;residency_policy")
    foreach(_field IN ITEMS recipe linker_script residency_check)
        _aros_native_get_source_path(_core_${_field} "${_core_json}" core "${_field}"
            "${_declared_input_paths}")
    endforeach()
    foreach(_field IN ITEMS resources libraries devices link_libraries)
        _aros_native_get_string_array(_core_${_field} "${_core_json}" core "${_field}")
    endforeach()
    _aros_native_get_token(_core_compiler_runtime_role "${_core_json}" core
        compiler_runtime_role)
    if(NOT _core_compiler_runtime_role STREQUAL "libgcc")
        _aros_native_contract_fail("core.compiler_runtime_role" "must be libgcc")
    endif()
    string(JSON _residency GET "${_core_json}" residency_policy)
    _aros_native_require_object_members("${_residency}" core.residency_policy
        "algorithm;section;flash_start;flash_end;sram_start;sram_end")
    _aros_native_get_string(_residency_algorithm "${_residency}" core.residency_policy algorithm)
    _aros_native_get_string(_residency_section "${_residency}" core.residency_policy section)
    if(NOT _residency_algorithm STREQUAL "riscv32-xip-v1" OR
       NOT _residency_section STREQUAL ".sramtext" OR
       NOT _abi_isa MATCHES "^rv32i")
        _aros_native_contract_fail("core.residency_policy"
            "unsupported native residency algorithm, section or width")
    endif()
    foreach(_field IN ITEMS flash_start flash_end sram_start sram_end)
        _aros_native_require_type("${_residency}" core.residency_policy "${_field}" NUMBER)
        string(JSON _residency_${_field} GET "${_residency}" "${_field}")
        if(NOT "${_residency_${_field}}" MATCHES "^[0-9]+$" OR
           _residency_${_field} GREATER 4294967296)
            _aros_native_contract_fail("core.residency_policy.${_field}"
                "must be an unsigned RV32 range bound")
        endif()
    endforeach()
    if(_residency_flash_start GREATER_EQUAL _residency_flash_end OR
       _residency_sram_start GREATER_EQUAL _residency_sram_end OR
       (_residency_flash_start LESS _residency_sram_end AND
        _residency_sram_start LESS _residency_flash_end))
        _aros_native_contract_fail("core.residency_policy"
            "ranges must be nonempty, disjoint RV32 half-open ranges")
    endif()

    string(JSON _package_json GET "${_contract_json}" package)
    _aros_native_require_object_members("${_package_json}" package
        "recipe;format;target;limit_from_board")
    _aros_native_get_source_path(_package_recipe "${_package_json}" package recipe
        "${_declared_input_paths}")
    _aros_native_get_string(_package_format "${_package_json}" package format)
    if(NOT _package_format STREQUAL "aros-pkg-v1")
        _aros_native_contract_fail("package.format" "must be aros-pkg-v1")
    endif()
    foreach(_field IN ITEMS target limit_from_board)
        _aros_native_get_token(_package_${_field} "${_package_json}" package "${_field}")
    endforeach()

    string(JSON _media_json GET "${_contract_json}" media)
    set(_media_geometry_contract "")
    set(_media_fields
        "chip;board_rules;partition_table;core_partition;package_partition;development_volume_offset_from_board;bootloader_configuration;bootloader_patch;idf_version")
    string(JSON _geometry_type ERROR_VARIABLE _geometry_error TYPE "${_media_json}" geometry_contract)
    if(_geometry_error STREQUAL "NOTFOUND")
        list(APPEND _media_fields geometry_contract)
    endif()
    _aros_native_require_object_members("${_media_json}" media
        "${_media_fields}")
    if(_geometry_error STREQUAL "NOTFOUND")
        _aros_native_get_source_path(_media_geometry_contract "${_media_json}" media geometry_contract
            "${_declared_input_paths}")
        set(_geometry_path "${_source_root}/${_media_geometry_contract}")
        file(SIZE "${_geometry_path}" _geometry_size)
        if(_geometry_size GREATER 1048576)
            _aros_native_contract_fail("media.geometry_contract"
                "exceeds the 1048576-byte limit")
        endif()
        file(READ "${_geometry_path}" _geometry_json LIMIT 1048577)
        string(LENGTH "${_geometry_json}" _geometry_read_length)
        if(_geometry_read_length GREATER 1048576)
            _aros_native_contract_fail("media.geometry_contract"
                "exceeds the 1048576-byte limit")
        endif()
        _aros_native_require_type("${_geometry_json}" "media.geometry_contract"
            package_limit_bytes NUMBER)
        string(JSON _package_limit_bytes GET "${_geometry_json}" package_limit_bytes)
        _aros_native_validate_positive_safe_unsigned(_package_limit_bytes
            "${_package_limit_bytes}" "media.geometry_contract.package_limit_bytes")
    endif()
    _aros_native_get_token(_media_chip "${_media_json}" media chip)
    foreach(_field IN ITEMS board_rules partition_table bootloader_configuration bootloader_patch)
        _aros_native_get_source_path(_media_${_field} "${_media_json}" media "${_field}"
            "${_declared_input_paths}")
    endforeach()
    foreach(_field IN ITEMS core_partition package_partition
            development_volume_offset_from_board idf_version)
        _aros_native_get_token(_media_${_field} "${_media_json}" media "${_field}")
    endforeach()

    # Store the successful validation identity in process-global state so the
    # compiler recheck and explicit build-tree lock can require this exact run.
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_SOURCE "${_source_root}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_PATH "${_contract_path}")
    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_CONTRACT_VALIDATED_SHA256 "${_actual_contract_sha}")

    set(AROS_NATIVE_BUILD_CONTRACT_PATH "${_contract_path}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CONTRACT_SHA256 "${_actual_contract_sha}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_SOURCE_DIR "${_source_root}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_SCHEMA_VERSION "${_schema_version}" PARENT_SCOPE)
    # Keep source defaults as data. Never inject arbitrary names into the
    # CMake namespace or convert them to compiler flags.
    set(AROS_NATIVE_BUILD_MAKE_VARIABLES_JSON "${_make_variables_json}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MAKE_INCLUDE_BINDINGS_JSON
        "${_make_include_bindings_json}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_PROFILE "${_profile}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_BOARD "${_board}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_SOURCE_BASELINE "${_source_baseline}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_QUALIFICATION "${_qualification}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_INPUT_PATHS "${_declared_input_paths}" PARENT_SCOPE)
    foreach(_index RANGE 0 ${_last_input})
        set(AROS_NATIVE_BUILD_INPUT_${_index}_PATH "${_input_${_index}_path}" PARENT_SCOPE)
        set(AROS_NATIVE_BUILD_INPUT_${_index}_SHA256 "${_input_${_index}_sha}" PARENT_SCOPE)
    endforeach()
    set(AROS_NATIVE_BUILD_ABI_SOURCE_CPU "${_abi_source_cpu}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_ABI_TARGET_TRIPLE "${_abi_target_triple}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_ABI_ISA "${_abi_isa}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_ABI "${_abi_abi}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_ABI_CODE_MODEL "${_abi_code_model}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_ABI_FLAVOUR "${_abi_flavour}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_ABI_PLATFORM_SMP "${_abi_platform_smp}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_EXEC_SMP "${_exec_smp}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_ABI_USE_MMU "${_abi_use_mmu}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CORE_RECIPE "${_core_recipe}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CORE_LINKER_SCRIPT "${_core_linker_script}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CORE_RESOURCES "${_core_resources}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CORE_LIBRARIES "${_core_libraries}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CORE_DEVICES "${_core_devices}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CORE_LINK_LIBRARIES "${_core_link_libraries}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CORE_COMPILER_RUNTIME_ROLE
        "${_core_compiler_runtime_role}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CORE_RESIDENCY_CHECK "${_core_residency_check}" PARENT_SCOPE)
    # Pass the entire exact source policy to the owned verifier, not an
    # executable profile hook. Region values never come from board-name code.
    set(AROS_NATIVE_BUILD_CORE_RESIDENCY_POLICY "${_residency}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_PACKAGE_RECIPE "${_package_recipe}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_PACKAGE_FORMAT "${_package_format}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_PACKAGE_TARGET "${_package_target}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_PACKAGE_LIMIT_FROM_BOARD "${_package_limit_from_board}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES "${_package_limit_bytes}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_CHIP "${_media_chip}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_BOARD_RULES "${_media_board_rules}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_PARTITION_TABLE "${_media_partition_table}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_CORE_PARTITION "${_media_core_partition}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_PACKAGE_PARTITION "${_media_package_partition}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_DEVELOPMENT_VOLUME_OFFSET_FROM_BOARD
        "${_media_development_volume_offset_from_board}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_BOOTLOADER_CONFIGURATION
        "${_media_bootloader_configuration}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_BOOTLOADER_PATCH "${_media_bootloader_patch}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_IDF_VERSION "${_media_idf_version}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_MEDIA_GEOMETRY_CONTRACT "${_media_geometry_contract}" PARENT_SCOPE)
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE PARENT_SCOPE)
endfunction()

function(aros_validate_native_build_compiler_target)
    _aros_native_require_current_validation(_source_root _contract_path _contract_sha)
    if(NOT DEFINED AROS_NATIVE_BUILD_ABI_ISA OR
       NOT DEFINED AROS_NATIVE_BUILD_ABI OR
       NOT DEFINED AROS_NATIVE_BUILD_ABI_CODE_MODEL)
        _aros_native_contract_fail("compiler target"
            "validated ABI values are unavailable in this directory scope")
    endif()
    if(NOT DEFINED AROS_GNU_TARGET_COMPILE_OPTIONS OR
       NOT AROS_GNU_TARGET_COMPILE_OPTIONS)
        _aros_native_contract_fail("compiler target"
            "AROS_GNU_TARGET_COMPILE_OPTIONS is missing after GNU toolchain initialization")
    endif()

    set(_march_count 0)
    set(_mabi_count 0)
    set(_mcmodel_count 0)
    foreach(_option IN LISTS AROS_GNU_TARGET_COMPILE_OPTIONS)
        if(_option MATCHES "^-march")
            math(EXPR _march_count "${_march_count} + 1")
            if(NOT _option STREQUAL "-march=${AROS_NATIVE_BUILD_ABI_ISA}")
                _aros_native_contract_fail("compiler target"
                    "GNU -march option differs from the validated source ISA")
            endif()
        elseif(_option MATCHES "^-mabi")
            math(EXPR _mabi_count "${_mabi_count} + 1")
            if(NOT _option STREQUAL "-mabi=${AROS_NATIVE_BUILD_ABI}")
                _aros_native_contract_fail("compiler target"
                    "GNU -mabi option differs from the validated source ABI")
            endif()
        elseif(_option MATCHES "^-mcmodel")
            math(EXPR _mcmodel_count "${_mcmodel_count} + 1")
            if(NOT _option STREQUAL "-mcmodel=${AROS_NATIVE_BUILD_ABI_CODE_MODEL}")
                _aros_native_contract_fail("compiler target"
                    "GNU -mcmodel option differs from the validated source code model")
            endif()
        endif()
    endforeach()
    if(NOT _march_count EQUAL 1 OR NOT _mabi_count EQUAL 1 OR
       NOT _mcmodel_count EQUAL 1)
        _aros_native_contract_fail("compiler target"
            "GNU target options must contain exactly one -march, -mabi, and -mcmodel")
    endif()

    set_property(GLOBAL PROPERTY AROS_NATIVE_BUILD_COMPILER_TARGET_VALIDATED TRUE)
    set(AROS_NATIVE_BUILD_COMPILER_TARGET_VALIDATED TRUE PARENT_SCOPE)
endfunction()

function(aros_lock_native_build_contract_identity)
    _aros_native_require_current_validation(_source_root _contract_path _contract_sha)

    set(_lock_source_defined FALSE)
    set(_lock_path_defined FALSE)
    set(_lock_sha_defined FALSE)
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_LOCKED_SOURCE_DIR)
        set(_lock_source_defined TRUE)
    endif()
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_LOCKED_PATH)
        set(_lock_path_defined TRUE)
    endif()
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_LOCKED_SHA256)
        set(_lock_sha_defined TRUE)
    endif()

    if(_lock_source_defined OR _lock_path_defined OR _lock_sha_defined)
        if(NOT _lock_source_defined OR NOT _lock_path_defined OR NOT _lock_sha_defined)
            _aros_native_contract_fail("identity lock"
                "cached source, path, and digest must be present together")
        endif()
        if(NOT AROS_NATIVE_BUILD_CONTRACT_LOCKED_SOURCE_DIR STREQUAL _source_root OR
           NOT AROS_NATIVE_BUILD_CONTRACT_LOCKED_PATH STREQUAL _contract_path OR
           NOT AROS_NATIVE_BUILD_CONTRACT_LOCKED_SHA256 STREQUAL _contract_sha)
            _aros_native_contract_fail("identity lock"
                "this build tree is already bound to another source contract")
        endif()
    endif()

    set(AROS_NATIVE_BUILD_CONTRACT_LOCKED_SOURCE_DIR "${_source_root}" CACHE INTERNAL
        "Canonical source root locked to this native contract" FORCE)
    set(AROS_NATIVE_BUILD_CONTRACT_LOCKED_PATH "${_contract_path}" CACHE INTERNAL
        "Canonical native build contract locked to this build tree" FORCE)
    set(AROS_NATIVE_BUILD_CONTRACT_LOCKED_SHA256 "${_contract_sha}" CACHE INTERNAL
        "Native build contract digest locked to this build tree" FORCE)
    set(AROS_NATIVE_BUILD_CONTRACT_IDENTITY_LOCKED TRUE PARENT_SCOPE)
endfunction()
