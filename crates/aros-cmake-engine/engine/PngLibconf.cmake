# Exact build-time counterpart for workbench/libs/png/mmakefile.src:95.
#
# libpng's prebuilt configuration header appears only after its archive has
# been fetched.  The historical recipe rewrites its one
# PNG_ERROR_NUMBERS_SUPPORTED line and writes the result solely to the target
# SDK include root.  Keep that product as an ordinary CMake output instead of
# making a configure-time placeholder.

include(CMakeParseArguments)

# aros_stage_pnglibconf(NAME <private-target> FETCH_TARGET <fetch-target>
#                       OWNER <existing-target> PORTS_DIR <dir>
#                       SDK_INCLUDE_DIR <dir>)
function(aros_stage_pnglibconf)
    set(oneValueArgs NAME FETCH_TARGET OWNER PORTS_DIR SDK_INCLUDE_DIR)
    cmake_parse_arguments(PNG "" "${oneValueArgs}" "" ${ARGN})

    if(PNG_UNPARSED_ARGUMENTS OR PNG_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "aros_stage_pnglibconf received malformed arguments")
    endif()
    foreach(_required IN ITEMS NAME FETCH_TARGET OWNER PORTS_DIR SDK_INCLUDE_DIR)
        if(NOT PNG_${_required})
            message(FATAL_ERROR "aros_stage_pnglibconf requires ${_required}")
        endif()
    endforeach()
    if(NOT TARGET "${PNG_FETCH_TARGET}")
        message(FATAL_ERROR
            "${PNG_NAME}: missing libpng fetch target ${PNG_FETCH_TARGET}")
    endif()
    if(NOT TARGET "${PNG_OWNER}")
        message(FATAL_ERROR
            "${PNG_NAME}: missing libpng generated target ${PNG_OWNER}")
    endif()
    if(TARGET "${PNG_NAME}")
        message(FATAL_ERROR "${PNG_NAME}: pnglibconf staging target already exists")
    endif()

    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE
        OUTPUT_VARIABLE _binary_root)
    cmake_path(ABSOLUTE_PATH PNG_PORTS_DIR NORMALIZE
        OUTPUT_VARIABLE _ports_root)
    cmake_path(ABSOLUTE_PATH PNG_SDK_INCLUDE_DIR NORMALIZE
        OUTPUT_VARIABLE _sdk_root)
    cmake_path(IS_PREFIX _binary_root "${_ports_root}" NORMALIZE _ports_inside)
    cmake_path(IS_PREFIX _binary_root "${_sdk_root}" NORMALIZE _sdk_inside)
    if(NOT _ports_inside OR NOT _sdk_inside)
        message(FATAL_ERROR
            "${PNG_NAME}: libpng paths must remain below ${_binary_root}")
    endif()

    set(_input
        "${_ports_root}/libpng/libpng-1.6.58/scripts/pnglibconf.h.prebuilt")
    set(_output "${_sdk_root}/pnglibconf.h")
    cmake_path(NORMAL_PATH _input)
    cmake_path(NORMAL_PATH _output)
    cmake_path(IS_PREFIX _ports_root "${_input}" NORMALIZE _input_inside)
    cmake_path(IS_PREFIX _sdk_root "${_output}" NORMALIZE _output_inside)
    if(NOT _input_inside OR NOT _output_inside)
        message(FATAL_ERROR "${PNG_NAME}: pnglibconf path escapes its build roots")
    endif()

    get_property(_fetch_stamp TARGET "${PNG_FETCH_TARGET}"
        PROPERTY AROS_FETCH_COMPLETION_STAMP)
    if(NOT _fetch_stamp)
        message(FATAL_ERROR
            "${PNG_NAME}: ${PNG_FETCH_TARGET} has no fetch completion stamp")
    endif()

    string(SHA256 _output_key "${_output}")
    get_property(_previous_owner GLOBAL PROPERTY
        "AROS_PNGLIBCONF_OWNER_${_output_key}")
    if(_previous_owner AND NOT _previous_owner STREQUAL PNG_NAME)
        message(FATAL_ERROR
            "${PNG_NAME}: ${_output} is already owned by ${_previous_owner}")
    endif()
    set_property(GLOBAL PROPERTY "AROS_PNGLIBCONF_OWNER_${_output_key}"
        "${PNG_NAME}")

    set(_writer "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/WritePngLibconf.cmake")
    get_filename_component(_output_dir "${_output}" DIRECTORY)
    add_custom_command(
        OUTPUT "${_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
        COMMAND "${CMAKE_COMMAND}"
            "-DBINARY_ROOT=${_binary_root}"
            "-DINPUT=${_input}"
            "-DOUTPUT=${_output}"
            -P "${_writer}"
        # The writer avoids replacing identical output, but a changed fetch
        # stamp must still advance this declared output for Make generators.
        COMMAND "${CMAKE_COMMAND}" -E touch "${_output}"
        DEPENDS "${_fetch_stamp}" "${_writer}"
        COMMENT "Generating libpng pnglibconf.h"
        VERBATIM)
    add_custom_target("${PNG_NAME}" DEPENDS "${_output}")
    add_dependencies("${PNG_OWNER}" "${PNG_NAME}")
endfunction()
