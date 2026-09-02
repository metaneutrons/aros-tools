# The HEIC MetaMake recipes configure two private headers after their selected
# port archives have been fetched.  They are consumed directly by C++ sources,
# so a phony `*-genfiles` target is not sufficient: Ninja needs a concrete
# producer for each generated pathname.

function(aros_stage_heic_version_header)
    set(oneValueArgs NAME FETCH_TARGET OWNER INPUT OUTPUT KIND)
    cmake_parse_arguments(HVH "" "${oneValueArgs}" "" ${ARGN})

    if(HVH_UNPARSED_ARGUMENTS OR HVH_KEYWORDS_MISSING_VALUES OR
       NOT HVH_NAME OR NOT HVH_FETCH_TARGET OR NOT HVH_OWNER OR
       NOT HVH_INPUT OR NOT HVH_OUTPUT OR NOT HVH_KIND)
        message(FATAL_ERROR
            "aros_stage_heic_version_header requires NAME, FETCH_TARGET, OWNER, "
            "INPUT, OUTPUT and KIND")
    endif()
    if(NOT HVH_KIND STREQUAL "DE265" AND NOT HVH_KIND STREQUAL "HEIF")
        message(FATAL_ERROR
            "${HVH_NAME}: unsupported HEIC version-header kind ${HVH_KIND}")
    endif()
    if(NOT TARGET "${HVH_FETCH_TARGET}" OR NOT TARGET "${HVH_OWNER}")
        message(FATAL_ERROR
            "${HVH_NAME}: HEIC version-header staging requires its fetch and owner targets")
    endif()

    cmake_path(ABSOLUTE_PATH HVH_INPUT NORMALIZE OUTPUT_VARIABLE _input)
    cmake_path(ABSOLUTE_PATH HVH_OUTPUT NORMALIZE OUTPUT_VARIABLE _output)
    cmake_path(ABSOLUTE_PATH AROS_PORTS_DIR NORMALIZE OUTPUT_VARIABLE _ports_root)
    cmake_path(ABSOLUTE_PATH AROS_GEN_DIR NORMALIZE OUTPUT_VARIABLE _gen_root)
    cmake_path(IS_PREFIX _ports_root "${_input}" NORMALIZE _input_is_port_file)
    cmake_path(IS_PREFIX _gen_root "${_output}" NORMALIZE _output_is_generated_file)
    if(NOT _input_is_port_file OR _input STREQUAL _ports_root)
        message(FATAL_ERROR
            "${HVH_NAME}: HEIC version-header input must be below AROS_PORTS_DIR: ${_input}")
    endif()
    if(NOT _output_is_generated_file OR _output STREQUAL _gen_root)
        message(FATAL_ERROR
            "${HVH_NAME}: HEIC version-header output must be below AROS_GEN_DIR: ${_output}")
    endif()

    get_property(_fetch_stamp TARGET "${HVH_FETCH_TARGET}" PROPERTY
        AROS_FETCH_COMPLETION_STAMP)
    if(NOT _fetch_stamp)
        message(FATAL_ERROR
            "${HVH_NAME}: ${HVH_FETCH_TARGET} has no fetch completion stamp")
    endif()

    set(_writer "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/WriteHeicVersionHeader.cmake")
    if(NOT EXISTS "${_writer}")
        message(FATAL_ERROR "${HVH_NAME}: missing HEIC version-header writer ${_writer}")
    endif()
    get_filename_component(_output_dir "${_output}" DIRECTORY)
    add_custom_command(
        OUTPUT "${_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
        COMMAND "${CMAKE_COMMAND}"
            "-DAROS_HEIC_VERSION_KIND=${HVH_KIND}"
            "-DAROS_HEIC_VERSION_INPUT=${_input}"
            "-DAROS_HEIC_VERSION_OUTPUT=${_output}"
            -P "${_writer}"
        DEPENDS "${_fetch_stamp}" "${_writer}"
        COMMENT "Generating AROS ${HVH_KIND} version header"
        VERBATIM)
    add_custom_target("${HVH_NAME}" DEPENDS "${_output}")
    add_dependencies("${HVH_OWNER}" "${HVH_NAME}")
endfunction()
