# Forward the Rust-owned Mesa configuration input into native graph
# invocations. This transports data only; it does not configure, validate, or
# authenticate Mesa configuration semantics.
function(aros_mesa_configuration_arguments out_var)
    if(DEFINED AROS_MESA_CONFIGURATION_MODE)
        if(AROS_MESA_CONFIGURATION_MODE STREQUAL "off")
            # Frontends select off explicitly on every unconfigured invocation,
            # so a reusable build cannot silently inherit an earlier input.
            unset(AROS_MESA_CONFIGURATION_INPUT CACHE)
            unset(AROS_MESA_CONFIGURATION_INPUT_SHA256 CACHE)
            set(${out_var} "" PARENT_SCOPE)
            return()
        elseif(NOT AROS_MESA_CONFIGURATION_MODE STREQUAL "bound")
            message(FATAL_ERROR "AROS_MESA_CONFIGURATION_MODE must be off or bound")
        endif()
    elseif(DEFINED AROS_MESA_CONFIGURATION_INPUT OR
           DEFINED AROS_MESA_CONFIGURATION_INPUT_SHA256)
        message(FATAL_ERROR "Mesa configuration input requires explicit bound mode")
    endif()
    set(_has_input FALSE)
    set(_has_sha256 FALSE)
    if(DEFINED AROS_MESA_CONFIGURATION_INPUT)
        set(_has_input TRUE)
    endif()
    if(DEFINED AROS_MESA_CONFIGURATION_INPUT_SHA256)
        set(_has_sha256 TRUE)
    endif()

    if(NOT _has_input AND NOT _has_sha256 AND
       NOT AROS_MESA_CONFIGURATION_MODE STREQUAL "bound")
        set(${out_var} "" PARENT_SCOPE)
        return()
    endif()
    if(NOT _has_input OR NOT _has_sha256 OR
       "${AROS_MESA_CONFIGURATION_INPUT}" STREQUAL "" OR
       "${AROS_MESA_CONFIGURATION_INPUT_SHA256}" STREQUAL "")
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT and "
            "AROS_MESA_CONFIGURATION_INPUT_SHA256 must both be defined and nonempty")
    endif()

    set(_path "${AROS_MESA_CONFIGURATION_INPUT}")
    set(_expected_sha256 "${AROS_MESA_CONFIGURATION_INPUT_SHA256}")
    string(LENGTH "${_expected_sha256}" _sha256_length)
    if(NOT _sha256_length EQUAL 64 OR
       NOT _expected_sha256 MATCHES "^[0-9a-f]+$")
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT_SHA256 must be 64 lowercase hexadecimal characters")
    endif()

    if(NOT IS_ABSOLUTE "${_path}")
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT must be an absolute path")
    endif()
    get_filename_component(_filename "${_path}" NAME)
    if(NOT _filename STREQUAL "mesa-transpiler-input.json")
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT must name mesa-transpiler-input.json")
    endif()
    if(IS_SYMLINK "${_path}")
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT must not be a symlink")
    endif()
    if(NOT EXISTS "${_path}" OR IS_DIRECTORY "${_path}")
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT must be an existing regular file")
    endif()

    file(REAL_PATH "${_path}" _canonical_path)
    if(NOT _canonical_path STREQUAL _path)
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT must be a canonical absolute path")
    endif()

    # Recompute on every call: the Rust parent owns this file and digest, and
    # may retain them across the two native graph invocations.
    file(SIZE "${_path}" _input_size)
    if(_input_size LESS 1 OR _input_size GREATER 1048576)
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT must be a nonempty bounded file")
    endif()
    file(SHA256 "${_path}" _actual_sha256)
    if(NOT _actual_sha256 STREQUAL _expected_sha256)
        message(FATAL_ERROR
            "AROS_MESA_CONFIGURATION_INPUT SHA-256 digest does not match the input bytes")
    endif()

    # Preserve a legal semicolon in the filename as one list element so the
    # caller still receives exactly four argv values when expanding the list.
    string(REPLACE ";" "\\;" _path_argument "${_path}")
    set(_arguments
        --mesa-configuration-input
        "${_path_argument}"
        --mesa-configuration-input-sha256
        "${_expected_sha256}")
    set(${out_var} "${_arguments}" PARENT_SCOPE)
endfunction()
