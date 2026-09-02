# Deterministic materialisation of the exact historic
#   $(ILBMTOC) $< >$@
# pattern-rule instances admitted by the transpiler.

function(aros_declare_ilbm_sources)
    set(oneValueArgs OWNER DIRECTORY)
    set(multiValueArgs INPUTS OUTPUTS)
    cmake_parse_arguments(ILBM "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(ILBM_UNPARSED_ARGUMENTS OR ILBM_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_declare_ilbm_sources: malformed arguments: "
            "${ILBM_UNPARSED_ARGUMENTS}${ILBM_KEYWORDS_MISSING_VALUES}")
    endif()
    foreach(_required OWNER DIRECTORY INPUTS OUTPUTS)
        if(NOT ILBM_${_required})
            message(FATAL_ERROR
                "aros_declare_ilbm_sources: ${_required} is required")
        endif()
    endforeach()
    if(ILBM_OWNER MATCHES "[^A-Za-z0-9_.+-]" OR
       ILBM_OWNER STREQUAL "." OR ILBM_OWNER STREQUAL "..")
        message(FATAL_ERROR
            "aros_declare_ilbm_sources: OWNER is not a safe target name: ${ILBM_OWNER}")
    endif()
    if(TARGET "${ILBM_OWNER}")
        message(FATAL_ERROR
            "aros_declare_ilbm_sources: OWNER already exists: ${ILBM_OWNER}")
    endif()
    if(NOT AROS_HOST_ILBMTOC OR NOT IS_ABSOLUTE "${AROS_HOST_ILBMTOC}")
        message(FATAL_ERROR
            "${ILBM_OWNER}: AROS_HOST_ILBMTOC must name the prefix-owned host tool")
    endif()

    list(LENGTH ILBM_INPUTS _input_count)
    list(LENGTH ILBM_OUTPUTS _output_count)
    if(NOT _input_count EQUAL _output_count OR _input_count LESS 1)
        message(FATAL_ERROR
            "${ILBM_OWNER}: INPUTS and OUTPUTS must contain equally many products")
    endif()

    cmake_path(ABSOLUTE_PATH CMAKE_SOURCE_DIR NORMALIZE OUTPUT_VARIABLE _source_root)
    if(IS_ABSOLUTE "${ILBM_DIRECTORY}")
        set(_directory "${ILBM_DIRECTORY}")
    else()
        set(_directory "${_source_root}/${ILBM_DIRECTORY}")
    endif()
    cmake_path(NORMAL_PATH _directory)
    cmake_path(IS_PREFIX _source_root "${_directory}" NORMALIZE _directory_contained)
    if(NOT _directory_contained OR _directory STREQUAL _source_root OR
       NOT IS_DIRECTORY "${_directory}")
        message(FATAL_ERROR
            "${ILBM_OWNER}: DIRECTORY must be an existing source-tree directory")
    endif()

    file(RELATIVE_PATH _declaring_rel "${_source_root}" "${_directory}")
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _binary_root)
    set(_generated_dir "${_binary_root}/gen/${_declaring_rel}")
    cmake_path(NORMAL_PATH _generated_dir)
    cmake_path(IS_PREFIX _binary_root "${_generated_dir}" NORMALIZE _generated_contained)
    if(NOT _generated_contained OR _generated_dir STREQUAL _binary_root)
        message(FATAL_ERROR
            "${ILBM_OWNER}: generated directory escapes the build tree")
    endif()

    set(_runner "${CMAKE_SOURCE_DIR}/cmake/RunIlbmToC.cmake")
    set(_products "")
    math(EXPR _last "${_input_count} - 1")
    foreach(_index RANGE 0 ${_last})
        list(GET ILBM_INPUTS ${_index} _input_rel)
        list(GET ILBM_OUTPUTS ${_index} _output_leaf)
        if(_input_rel MATCHES "[;$]" OR IS_ABSOLUTE "${_input_rel}")
            message(FATAL_ERROR
                "${ILBM_OWNER}: unsafe relative ILBM input: ${_input_rel}")
        endif()
        set(_input "${_directory}/${_input_rel}")
        cmake_path(NORMAL_PATH _input)
        cmake_path(IS_PREFIX _directory "${_input}" NORMALIZE _input_contained)
        if(NOT _input_contained OR _input STREQUAL _directory OR
           NOT EXISTS "${_input}" OR IS_DIRECTORY "${_input}")
            message(FATAL_ERROR
                "${ILBM_OWNER}: ILBM input is missing or escapes DIRECTORY: ${_input_rel}")
        endif()
        if(_output_leaf MATCHES "[/\\;$]" OR
           NOT _output_leaf MATCHES "^[A-Za-z0-9_.+-]+\\.c$")
            message(FATAL_ERROR
                "${ILBM_OWNER}: unsafe generated C filename: ${_output_leaf}")
        endif()
        set(_output "${_generated_dir}/${_output_leaf}")
        add_custom_command(
            OUTPUT "${_output}"
            COMMAND "${CMAKE_COMMAND}" -E make_directory "${_generated_dir}"
            COMMAND "${CMAKE_COMMAND}"
                "-DTOOL=${AROS_HOST_ILBMTOC}"
                "-DINPUT=${_input}"
                "-DOUTPUT=${_output}"
                "-DBINARY_ROOT=${_binary_root}"
                -P "${_runner}"
            DEPENDS "${AROS_HOST_ILBMTOC}" "${_input}" "${_runner}"
            COMMENT "Embedding ${_input_rel} as ${_output_leaf}"
            VERBATIM)
        list(APPEND _products "${_output}")
    endforeach()

    add_custom_target("${ILBM_OWNER}" DEPENDS ${_products})
    aros_gate_arch("${ILBM_OWNER}" "${_directory}")
    set_property(TARGET "${ILBM_OWNER}" PROPERTY
        AROS_GENERATED_INCLUDE_DIRECTORY "${_generated_dir}")
endfunction()
