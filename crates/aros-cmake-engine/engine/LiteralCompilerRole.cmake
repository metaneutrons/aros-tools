include_guard(GLOBAL)

# Direct compiler calls cannot silently ignore CMake metadata which would
# change the selected frontend or driver environment.
function(_aros_literal_validate_compiler_metadata language)
    set(_external_toolchain "${CMAKE_${language}_COMPILER_EXTERNAL_TOOLCHAIN}")
    if(NOT "${_external_toolchain}" STREQUAL "")
        message(FATAL_ERROR
            "Literal object: external compiler toolchains are not supported")
    endif()
    set(_frontend "${CMAKE_${language}_COMPILER_FRONTEND_VARIANT}")
    if(NOT "${_frontend}" STREQUAL "" AND NOT "${_frontend}" STREQUAL "GNU")
        message(FATAL_ERROR
            "Literal object: unsupported ${language} compiler frontend metadata")
    endif()
    foreach(_sysroot_variable IN ITEMS CMAKE_SYSROOT CMAKE_SYSROOT_COMPILE CMAKE_SYSROOT_LINK)
        if(DEFINED ${_sysroot_variable} AND NOT "${${_sysroot_variable}}" STREQUAL "")
            message(FATAL_ERROR
                "Literal object: external compiler sysroot metadata is not supported")
        endif()
    endforeach()
endfunction()

# Materialize CMake's explicit Clang language target. Native SDK consumers
# additionally bind it to the validated source triple.
function(_aros_literal_compiler_target_arguments out_arguments language expected_target)
    _aros_literal_validate_compiler_metadata("${language}")
    set(_target "${CMAKE_${language}_COMPILER_TARGET}")
    if(NOT "${expected_target}" STREQUAL "" AND
       NOT "${_target}" STREQUAL "${expected_target}")
        message(FATAL_ERROR
            "Literal object: ${language} compiler target differs from the selected source triple")
    endif()
    if("${_target}" STREQUAL "")
        if(NOT "${expected_target}" STREQUAL "")
            message(FATAL_ERROR
                "Literal object: ${language} compiler target is required by the selected source triple")
        endif()
        set(${out_arguments} "" PARENT_SCOPE)
        return()
    endif()
    if(NOT CMAKE_${language}_COMPILER_ID MATCHES "^(Clang|AppleClang)$")
        message(FATAL_ERROR
            "Literal object: ${language} compiler target metadata is unsupported for this driver")
    endif()

    if(NOT "${_target}" MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*$")
        message(FATAL_ERROR "Literal object: unsafe ${language} compiler target metadata")
    endif()
    set(${out_arguments} "--target=${_target}" PARENT_SCOPE)
endfunction()

function(_aros_literal_validate_native_target_arguments language source_arguments)
    foreach(_argument IN LISTS source_arguments)
        if(_argument MATCHES "^(-target|--target)(=|$)" OR
           _argument MATCHES "^--?(gcc-toolchain)(=|$)" OR
           _argument MATCHES "^-m(32|64)$")
            message(FATAL_ERROR
                "Literal object: native source arguments cannot override compiler target or pointer width")
        endif()
        if(_argument MATCHES "^-m(big-endian|little-endian|arm|thumb|thumb-interwork)$")
            message(FATAL_ERROR
                "Literal object: native source arguments contain an undeclared endian or instruction-mode override")
        endif()
    endforeach()
endfunction()

function(_aros_literal_native_sysroot_arguments out_arguments source_arguments)
    if(NOT IS_ABSOLUTE "${AROS_TARGET_SYSROOT}" OR
       NOT IS_ABSOLUTE "${AROS_BUILD_DIR}")
        message(FATAL_ERROR "Literal object: selected native sysroot is required")
    endif()
    cmake_path(ABSOLUTE_PATH AROS_TARGET_SYSROOT NORMALIZE OUTPUT_VARIABLE _sysroot)
    cmake_path(IS_PREFIX AROS_BUILD_DIR "${_sysroot}" NORMALIZE _owned)
    if(NOT _owned OR _sysroot STREQUAL AROS_BUILD_DIR)
        message(FATAL_ERROR "Literal object: selected native sysroot escaped the build")
    endif()
    set(_count 0)
    set(_expect_path FALSE)
    foreach(_argument IN LISTS source_arguments)
        if(_expect_path)
            set(_path "${_argument}")
            set(_expect_path FALSE)
        elseif(_argument STREQUAL "--sysroot" OR _argument STREQUAL "-isysroot")
            math(EXPR _count "${_count} + 1")
            set(_expect_path TRUE)
            continue()
        elseif(_argument MATCHES "^--sysroot=(.*)$")
            math(EXPR _count "${_count} + 1")
            set(_path "${CMAKE_MATCH_1}")
        else()
            continue()
        endif()
        if(NOT IS_ABSOLUTE "${_path}")
            message(FATAL_ERROR "Literal object: native source sysroot must be the selected absolute path")
        endif()
        cmake_path(ABSOLUTE_PATH _path NORMALIZE OUTPUT_VARIABLE _path)
        if(NOT _path STREQUAL _sysroot)
            message(FATAL_ERROR "Literal object: native source sysroot differs from the selected Developer tree")
        endif()
    endforeach()
    if(_expect_path OR _count GREATER 1)
        message(FATAL_ERROR "Literal object: requires at most one complete selected native sysroot argument")
    endif()
    if(_count EQUAL 0)
        set(${out_arguments} "--sysroot=${_sysroot}" PARENT_SCOPE)
    else()
        set(${out_arguments} "" PARENT_SCOPE)
    endif()
endfunction()

function(_aros_literal_sdk_role_options out_options language source_arguments)
    _aros_literal_validate_native_target_arguments("${language}" "${source_arguments}")
    set(_role_flags)
    foreach(_argument IN LISTS source_arguments)
        if(_argument MATCHES "^-mabi(=|$)" OR
           _argument MATCHES "^-m(soft-float|hard-float)$" OR
           _argument MATCHES "^-m(no-)?strict-align$")
            message(FATAL_ERROR
                "Literal object: native SDK source arguments contain an unsupported or undeclared ABI option")
        elseif(_argument MATCHES "^(-m(arch|cpu|fpu|cmodel|float-abi)(=.*)?|-m(no-)?red-zone|-f(no-)?(pic|PIC|pie|PIE))$")
            list(APPEND _role_flags "${_argument}")
        endif()
    endforeach()

    # The SDK helpers accept shell-style flag strings, so pass only the
    # individually identified role flags. Macro argv (including values with
    # spaces) never enters their join/reparse path.
    string(JOIN " " _role_flags_text ${_role_flags})
    _aros_sdk_compiler_options_to_append(_missing_options
        "${_role_flags_text}" "${language}")
    _aros_sdk_reconcile_float_abi(_with_float_abi
        "${_role_flags_text}" "${language}" TRUE)

    set(_options ${_missing_options})
    if(NOT _with_float_abi STREQUAL _role_flags_text)
        get_property(_float_abi GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_FLOAT_ABI)
        if(NOT _float_abi STREQUAL "")
            list(APPEND _options "-mfloat-abi=${_float_abi}")
        endif()
    endif()
    set(${out_options} "${_options}" PARENT_SCOPE)
endfunction()

function(_aros_literal_gnu_role_options out_options language source_arguments)
    _aros_literal_validate_compiler_metadata("${language}")
    _aros_literal_validate_native_target_arguments("${language}" "${source_arguments}")
    if(NOT CMAKE_${language}_COMPILER_ID STREQUAL "GNU")
        message(FATAL_ERROR
            "Literal object: native GNU ABI options require the selected GNU ${language} driver")
    endif()
    if(NOT "${CMAKE_${language}_COMPILER_TARGET}" STREQUAL "")
        message(FATAL_ERROR
            "Literal object: GNU compiler target metadata is unsupported for the direct driver")
    endif()
    if(NOT DEFINED AROS_GNU_TARGET_COMPILE_OPTIONS OR
       NOT AROS_GNU_TARGET_COMPILE_OPTIONS)
        message(FATAL_ERROR
            "Literal object: validated GNU target compile options are required")
    endif()

    # Each ABI dimension is a single selected option. Repeating even the same
    # option is not a source declaration that this adapter can normalize.
    set(_selected_keys)
    foreach(_selected IN LISTS AROS_GNU_TARGET_COMPILE_OPTIONS)
        if(NOT _selected MATCHES "^-(march|mabi|mcmodel)=[A-Za-z0-9_.+-]+$" AND
           NOT _selected MATCHES "^-m(no-)?strict-align$")
            message(FATAL_ERROR
                "Literal object: unsupported validated GNU compiler-role option")
        endif()
        string(REGEX REPLACE "=.*$" "" _key "${_selected}")
        string(REPLACE "-mno-strict-align" "-mstrict-align" _key "${_key}")
        if(_key IN_LIST _selected_keys)
            message(FATAL_ERROR "Literal object: duplicate validated GNU ABI option")
        endif()
        list(APPEND _selected_keys "${_key}")
    endforeach()

    set(_options)
    set(_source_abi_keys)
    foreach(_argument IN LISTS source_arguments)
        if(_argument MATCHES "^-m(arch|abi|cmodel|cpu|fpu|float-abi|soft-float|hard-float)(=|$)" OR
           _argument MATCHES "^-m(no-)?strict-align$" OR
           _argument MATCHES "^-m(32|64)$")
            set(_selected_found FALSE)
            foreach(_selected IN LISTS AROS_GNU_TARGET_COMPILE_OPTIONS)
                if(_argument STREQUAL _selected)
                    set(_selected_found TRUE)
                endif()
            endforeach()
            if(NOT _selected_found)
                message(FATAL_ERROR
                    "Literal object: native GNU source ABI option conflicts with the validated target")
            endif()
            string(REGEX REPLACE "=.*$" "" _key "${_argument}")
            string(REPLACE "-mno-strict-align" "-mstrict-align" _key "${_key}")
            if(_key IN_LIST _source_abi_keys)
                message(FATAL_ERROR "Literal object: duplicate source GNU ABI option")
            endif()
            list(APPEND _source_abi_keys "${_key}")
        endif()
    endforeach()

    foreach(_selected IN LISTS AROS_GNU_TARGET_COMPILE_OPTIONS)
        if(NOT _selected IN_LIST source_arguments)
            list(APPEND _options "${_selected}")
        endif()
    endforeach()
    set(${out_options} "${_options}" PARENT_SCOPE)
endfunction()

function(_aros_literal_native_consumer_compiler_role out_role)
    get_property(_schema GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA)
    if(_schema STREQUAL "aros-sdk-consumer-validation-v1")
        set(_role sdk)
    elseif(_schema STREQUAL "aros-native-consumer-validation-v1")
        set(_role gnu)
    else()
        message(FATAL_ERROR
            "Literal object: unsupported validated native consumer schema")
    endif()
    set(${out_role} "${_role}" PARENT_SCOPE)
endfunction()

function(_aros_literal_compiler_role_arguments out_arguments language source_arguments native_source_selected)
    set(_options)
    set(_native_role "")
    if(native_source_selected)
        get_property(_schema GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA)
        if(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED OR NOT "${_schema}" STREQUAL "")
            _aros_literal_native_consumer_compiler_role(_native_role)
            if(NOT COMMAND aros_validate_native_consumer_compiler_target)
                message(FATAL_ERROR
                    "Literal object: native consumer compiler-role validator is unavailable")
            endif()
            aros_validate_native_consumer_compiler_target()
        elseif(AROS_NATIVE_BUILD_CONTRACT_VALIDATED)
            if(NOT COMMAND aros_validate_native_build_compiler_target)
                message(FATAL_ERROR
                    "Literal object: native build compiler-role validator is unavailable")
            endif()
            aros_validate_native_build_compiler_target()
            set(_native_role "gnu")
        else()
            message(FATAL_ERROR
                "Literal object: selected native source has no validated compiler role")
        endif()
    endif()

    if(_native_role STREQUAL "sdk")
        if(NOT CMAKE_${language}_COMPILER_ID STREQUAL "Clang")
            message(FATAL_ERROR
                "Literal object: native SDK requires the selected Clang ${language} driver")
        endif()
        _aros_literal_compiler_target_arguments(_target_arguments "${language}"
            "${AROS_TARGET_TRIPLE}")
        list(APPEND _options "${_target_arguments}")
        _aros_literal_sdk_role_options(_abi_options "${language}" "${source_arguments}")
        list(APPEND _options ${_abi_options})
    elseif(_native_role STREQUAL "gnu")
        _aros_literal_gnu_role_options(_abi_options "${language}" "${source_arguments}")
        list(APPEND _options ${_abi_options})
    else()
        if(NOT "${CMAKE_${language}_COMPILER_TARGET}" STREQUAL "")
            _aros_literal_validate_native_target_arguments(
                "${language}" "${source_arguments}")
        endif()
        _aros_literal_compiler_target_arguments(_target_arguments "${language}" "")
        list(APPEND _options "${_target_arguments}")
    endif()

    if(NOT _native_role STREQUAL "")
        _aros_literal_native_sysroot_arguments(_sysroot_arguments "${source_arguments}")
        list(APPEND _options "${_sysroot_arguments}")
    endif()

    set(_filtered_options)
    foreach(_option IN LISTS _options)
        if(NOT "${_option}" STREQUAL "")
            list(APPEND _filtered_options "${_option}")
        endif()
    endforeach()
    set(${out_arguments} "${_filtered_options}" PARENT_SCOPE)
endfunction()
