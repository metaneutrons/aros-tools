# The upstream FreeType MetaMake rule deliberately does not publish the
# archive's default `ftoption.h`: it rewrites five option-bearing source lines
# into the AROS SDK configuration first.  Keep that as a real, fetch-dependent
# output instead of exposing the port source directory as an include path.
# CONSUMERS names targets whose compilation may include the staged header; the
# explicit ordering is required because an include path alone is not a build
# dependency in CMake.

function(aros_stage_freetype_options)
    set(oneValueArgs NAME FETCH_TARGET OWNER INPUT OUTPUT)
    set(multiValueArgs CONSUMERS)
    cmake_parse_arguments(FTO "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(FTO_UNPARSED_ARGUMENTS OR FTO_KEYWORDS_MISSING_VALUES OR
       NOT FTO_NAME OR NOT FTO_FETCH_TARGET OR NOT FTO_OWNER OR
       NOT FTO_INPUT OR NOT FTO_OUTPUT)
        message(FATAL_ERROR
            "aros_stage_freetype_options requires NAME, FETCH_TARGET, OWNER, "
            "INPUT and OUTPUT")
    endif()
    if(NOT TARGET "${FTO_FETCH_TARGET}" OR NOT TARGET "${FTO_OWNER}")
        message(FATAL_ERROR
            "${FTO_NAME}: FreeType option staging requires its fetch and owner targets")
    endif()

    cmake_path(ABSOLUTE_PATH FTO_INPUT NORMALIZE OUTPUT_VARIABLE _input)
    cmake_path(ABSOLUTE_PATH FTO_OUTPUT NORMALIZE OUTPUT_VARIABLE _output)
    cmake_path(ABSOLUTE_PATH AROS_PORTS_DIR NORMALIZE OUTPUT_VARIABLE _ports_root)
    cmake_path(ABSOLUTE_PATH AROS_SDK_INCLUDE_DIR NORMALIZE
        OUTPUT_VARIABLE _sdk_include_root)
    cmake_path(IS_PREFIX _ports_root "${_input}" NORMALIZE _input_is_port_file)
    cmake_path(IS_PREFIX _sdk_include_root "${_output}" NORMALIZE _output_is_sdk_file)
    if(NOT _input_is_port_file OR _input STREQUAL _ports_root)
        message(FATAL_ERROR
            "${FTO_NAME}: FreeType option input must be below AROS_PORTS_DIR: ${_input}")
    endif()
    if(NOT _output_is_sdk_file OR _output STREQUAL _sdk_include_root)
        message(FATAL_ERROR
            "${FTO_NAME}: FreeType option output must be below the SDK include root: "
            "${_output}")
    endif()

    get_property(_fetch_stamp TARGET "${FTO_FETCH_TARGET}" PROPERTY
        AROS_FETCH_COMPLETION_STAMP)
    if(NOT _fetch_stamp)
        message(FATAL_ERROR
            "${FTO_NAME}: ${FTO_FETCH_TARGET} has no fetch completion stamp")
    endif()

    set(_writer "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/WriteFreetypeOptions.cmake")
    if(NOT EXISTS "${_writer}")
        message(FATAL_ERROR "${FTO_NAME}: missing FreeType option writer ${_writer}")
    endif()
    get_filename_component(_output_dir "${_output}" DIRECTORY)
    add_custom_command(
        OUTPUT "${_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
        COMMAND "${CMAKE_COMMAND}"
            "-DAROS_FREETYPE_OPTIONS_INPUT=${_input}"
            "-DAROS_FREETYPE_OPTIONS_OUTPUT=${_output}"
            -P "${_writer}"
        DEPENDS "${_fetch_stamp}" "${_writer}"
        COMMENT "Generating AROS FreeType build options"
        VERBATIM)
    add_custom_target("${FTO_NAME}" DEPENDS "${_output}")
    add_dependencies("${FTO_OWNER}" "${FTO_NAME}")
    foreach(_consumer IN LISTS FTO_CONSUMERS)
        if(NOT TARGET "${_consumer}")
            message(FATAL_ERROR
                "${FTO_NAME}: FreeType option consumer does not exist: ${_consumer}")
        endif()
        add_dependencies("${_consumer}" "${FTO_NAME}")
    endforeach()
endfunction()
