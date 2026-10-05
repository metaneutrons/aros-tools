include_guard(GLOBAL)

# Materialise a source-proven pkg-config-style SDK text product. The only
# supported operations are encoded literals from the scanner; this function
# never invokes Make, sed, a shell, or a source-provided command.
function(aros_transform_sdk_text)
    set(oneValueArgs NAME INPUT OUTPUT FETCH FILE FILE_SHA256)
    set(multiValueArgs OPERATIONS)
    cmake_parse_arguments(PARSE_ARGV 0 ST "" "${oneValueArgs}" "${multiValueArgs}")

    if(ST_UNPARSED_ARGUMENTS OR ST_KEYWORDS_MISSING_VALUES OR
       NOT ST_NAME OR NOT ST_INPUT OR NOT ST_OUTPUT OR NOT ST_FETCH OR
       NOT ST_OPERATIONS)
        message(FATAL_ERROR
            "aros_transform_sdk_text requires NAME, INPUT, OUTPUT, FETCH and OPERATIONS")
    endif()
    if(NOT ST_NAME MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*$")
        message(FATAL_ERROR "SDK text owner is not one safe target name: ${ST_NAME}")
    endif()
    if(NOT TARGET "${ST_FETCH}")
        message(FATAL_ERROR "${ST_NAME}: missing SDK text fetch target ${ST_FETCH}")
    endif()
    if(NOT AROS_DEVELOPER_LIB_DIR)
        message(FATAL_ERROR "${ST_NAME}: AROS_DEVELOPER_LIB_DIR is not configured")
    endif()

    set(_declaration_dependencies "")
    set(_declaration_arguments "")
    if(ST_FILE OR ST_FILE_SHA256)
        if(NOT ST_FILE OR NOT ST_FILE_SHA256 OR NOT AROS_SOURCE_DIR)
            message(FATAL_ERROR "${ST_NAME}: SDK text source binding requires FILE, FILE_SHA256 and AROS_SOURCE_DIR")
        endif()
        string(LENGTH "${ST_FILE_SHA256}" _digest_length)
        if(NOT _digest_length EQUAL 64 OR NOT ST_FILE_SHA256 MATCHES "^[0-9a-f]+$" OR
           IS_ABSOLUTE "${ST_FILE}" OR NOT ST_FILE MATCHES "^[A-Za-z0-9_.+-]+(/[A-Za-z0-9_.+-]+)*$" OR
           ST_FILE MATCHES "(^|/)[.][.]?(/|$)")
            message(FATAL_ERROR "${ST_NAME}: SDK text source binding is invalid")
        endif()
        file(REAL_PATH "${AROS_SOURCE_DIR}" _source_root)
        set(_declaration "${_source_root}/${ST_FILE}")
        set(_probe "${_source_root}")
        string(REPLACE "/" ";" _components "${ST_FILE}")
        foreach(_component IN LISTS _components)
            set(_probe "${_probe}/${_component}")
            if(IS_SYMLINK "${_probe}")
                message(FATAL_ERROR "${ST_NAME}: SDK text source path crosses a symlink")
            endif()
        endforeach()
        find_program(_source_test NAMES test PATHS /usr/bin /bin NO_DEFAULT_PATH NO_CACHE REQUIRED)
        execute_process(COMMAND "${_source_test}" -f "${_declaration}"
            RESULT_VARIABLE _regular_source TIMEOUT 10)
        if(NOT "${_regular_source}" STREQUAL "0")
            message(FATAL_ERROR "${ST_NAME}: SDK text declaration is not a regular file")
        endif()
        file(SHA256 "${_declaration}" _source_sha)
        if(NOT _source_sha STREQUAL ST_FILE_SHA256)
            message(FATAL_ERROR "${ST_NAME}: SDK text source changed after translation")
        endif()
        set(_declaration_dependencies "${_declaration}")
        set(_declaration_arguments
            "-DDECLARATION_SOURCE=${_declaration}"
            "-DDECLARATION_SOURCE_ROOT=${_source_root}"
            "-DDECLARATION_SHA256=${ST_FILE_SHA256}")
        set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${_declaration}")
    endif()

    foreach(_path IN ITEMS ST_INPUT ST_OUTPUT AROS_DEVELOPER_LIB_DIR CMAKE_BINARY_DIR)
        if("${${_path}}" MATCHES "[;\"\r\n|&]")
            message(FATAL_ERROR "${ST_NAME}: unsafe path argument for ${_path}")
        endif()
        if("${${_path}}" MATCHES "(^|/)[.][.]?(/|$)")
            message(FATAL_ERROR "${ST_NAME}: path argument for ${_path} contains traversal")
        endif()
    endforeach()

    cmake_path(ABSOLUTE_PATH ST_INPUT NORMALIZE OUTPUT_VARIABLE _input)
    cmake_path(ABSOLUTE_PATH ST_OUTPUT NORMALIZE OUTPUT_VARIABLE _output)
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _binary_root)
    cmake_path(ABSOLUTE_PATH AROS_DEVELOPER_LIB_DIR NORMALIZE
        OUTPUT_VARIABLE _developer_lib_root)
    set(_pkgconfig_root "${_developer_lib_root}/pkgconfig")

    if(NOT IS_ABSOLUTE "${ST_INPUT}" OR NOT IS_ABSOLUTE "${ST_OUTPUT}")
        message(FATAL_ERROR "${ST_NAME}: SDK text input and output must be absolute paths")
    endif()
    cmake_path(IS_PREFIX _binary_root "${_developer_lib_root}" NORMALIZE
        _developer_lib_inside_binary)
    if(NOT _developer_lib_inside_binary)
        message(FATAL_ERROR
            "${ST_NAME}: configured Developer/lib root escapes the build directory")
    endif()
    cmake_path(IS_PREFIX _pkgconfig_root "${_output}" NORMALIZE _output_inside_pkgconfig)
    if(NOT _output_inside_pkgconfig OR _output STREQUAL _pkgconfig_root)
        message(FATAL_ERROR
            "${ST_NAME}: SDK text output must be below Developer/lib/pkgconfig: ${_output}")
    endif()
    cmake_path(GET _output FILENAME _output_name)
    if(NOT _output_name MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*[.]pc$")
        message(FATAL_ERROR "${ST_NAME}: SDK text output must be one safe `.pc` file")
    endif()
    if(_input STREQUAL _output)
        message(FATAL_ERROR "${ST_NAME}: SDK text input and output are identical")
    endif()

    # The fetch target is part of the producer contract, not merely a target
    # ordering hint. The input must be below its declared extraction root, and
    # the content-locked completion stamp is the file-level prerequisite.
    get_property(_fetch_destination TARGET "${ST_FETCH}"
        PROPERTY AROS_FETCH_DESTINATION)
    get_property(_fetch_stamp TARGET "${ST_FETCH}"
        PROPERTY AROS_FETCH_COMPLETION_STAMP)
    if(NOT _fetch_destination OR NOT _fetch_stamp)
        message(FATAL_ERROR
            "${ST_NAME}: fetch target ${ST_FETCH} lacks destination/completion-stamp properties")
    endif()
    cmake_path(ABSOLUTE_PATH _fetch_destination NORMALIZE
        OUTPUT_VARIABLE _fetch_destination)
    if(NOT IS_ABSOLUTE "${_fetch_destination}" OR
       "${_fetch_destination}" MATCHES "[;\"\r\n|&]" OR
       "${_fetch_destination}" MATCHES "(^|/)[.][.]?(/|$)")
        message(FATAL_ERROR "${ST_NAME}: unsafe fetch destination ${_fetch_destination}")
    endif()
    cmake_path(IS_PREFIX _binary_root "${_fetch_destination}" NORMALIZE
        _fetch_destination_inside_binary)
    if(NOT _fetch_destination_inside_binary OR
       _fetch_destination STREQUAL _binary_root)
        message(FATAL_ERROR
            "${ST_NAME}: fetch destination escapes the build directory")
    endif()
    cmake_path(IS_PREFIX _fetch_destination "${_input}" NORMALIZE _input_below_fetch)
    if(NOT _input_below_fetch OR _input STREQUAL _fetch_destination)
        message(FATAL_ERROR
            "${ST_NAME}: SDK text input is not below fetch destination ${_fetch_destination}")
    endif()

    # The input will usually not exist until the build phase, so inspect every
    # currently existing component and repeat the same check in the runner.
    file(RELATIVE_PATH _relative_fetch_destination
        "${_binary_root}" "${_fetch_destination}")
    string(REPLACE "/" ";" _fetch_components "${_relative_fetch_destination}")
    set(_fetch_probe "${_binary_root}")
    foreach(_component IN LISTS _fetch_components)
        set(_fetch_probe "${_fetch_probe}/${_component}")
        if(IS_SYMLINK "${_fetch_probe}")
            message(FATAL_ERROR
                "${ST_NAME}: fetch destination crosses a symlink: ${_fetch_probe}")
        endif()
    endforeach()
    file(REAL_PATH "${_binary_root}" _binary_real)
    set(_existing_fetch "${_fetch_destination}")
    while(NOT EXISTS "${_existing_fetch}")
        cmake_path(GET _existing_fetch PARENT_PATH _existing_fetch_parent)
        if(_existing_fetch_parent STREQUAL _existing_fetch)
            message(FATAL_ERROR "${ST_NAME}: fetch destination has no existing ancestor")
        endif()
        set(_existing_fetch "${_existing_fetch_parent}")
    endwhile()
    file(REAL_PATH "${_existing_fetch}" _existing_fetch_real)
    cmake_path(IS_PREFIX _binary_real "${_existing_fetch_real}" NORMALIZE
        _fetch_physically_inside_binary)
    if(NOT _fetch_physically_inside_binary)
        message(FATAL_ERROR
            "${ST_NAME}: physical fetch destination escapes the build directory")
    endif()

    if(EXISTS "${_input}" AND (IS_DIRECTORY "${_input}" OR IS_SYMLINK "${_input}"))
        message(FATAL_ERROR "${ST_NAME}: SDK text input is not a regular file: ${_input}")
    endif()

    # Reject lexical aliases and every physical symlink along the output path.
    # Checking only the leaf would let a symlinked ancestor escape the build.
    file(RELATIVE_PATH _relative_output "${_binary_root}" "${_output}")
    if(_relative_output MATCHES "(^|/)[.][.]?(/|$)")
        message(FATAL_ERROR "${ST_NAME}: output escapes the binary directory")
    endif()
    string(REPLACE "/" ";" _output_components "${_relative_output}")
    set(_output_probe "${_binary_root}")
    list(LENGTH _output_components _output_component_count)
    set(_output_component_index 0)
    foreach(_component IN LISTS _output_components)
        math(EXPR _output_component_index "${_output_component_index} + 1")
        set(_output_probe "${_output_probe}/${_component}")
        if(IS_SYMLINK "${_output_probe}")
            message(FATAL_ERROR
                "${ST_NAME}: output path crosses a symlink: ${_output_probe}")
        endif()
        if(EXISTS "${_output_probe}" AND
           _output_component_index LESS _output_component_count AND
           NOT IS_DIRECTORY "${_output_probe}")
            message(FATAL_ERROR
                "${ST_NAME}: output parent is not a directory: ${_output_probe}")
        endif()
    endforeach()
    file(REAL_PATH "${_binary_root}" _binary_real)
    set(_existing_output "${_output}")
    while(NOT EXISTS "${_existing_output}")
        cmake_path(GET _existing_output PARENT_PATH _existing_output)
    endwhile()
    file(REAL_PATH "${_existing_output}" _output_real)
    cmake_path(IS_PREFIX _binary_real "${_output_real}" NORMALIZE _output_physically_inside)
    if(NOT _output_physically_inside)
        message(FATAL_ERROR "${ST_NAME}: physical output escapes the binary directory")
    endif()
    if(EXISTS "${_output}" AND IS_DIRECTORY "${_output}")
        message(FATAL_ERROR "${ST_NAME}: SDK text output is a directory")
    endif()

    foreach(_operation IN LISTS ST_OPERATIONS)
        _aros_validate_sdk_text_operation("${ST_NAME}" "${_operation}")
    endforeach()

    # A graph-selected provider may already have an aggregate target bearing
    # its Make name. Reuse only a target explicitly marked as this owner.
    string(SHA256 _owner_key "${ST_NAME}")
    get_property(_aggregate_owner GLOBAL PROPERTY
        "AROS_SDK_TEXT_AGGREGATE_${_owner_key}")
    if(TARGET "${ST_NAME}" AND NOT "${_aggregate_owner}" STREQUAL "${ST_NAME}")
        message(FATAL_ERROR "${ST_NAME}: SDK text owner target is already declared")
    elseif(NOT TARGET "${ST_NAME}")
        add_custom_target("${ST_NAME}")
        set_property(GLOBAL PROPERTY "AROS_SDK_TEXT_AGGREGATE_${_owner_key}" "${ST_NAME}")
    endif()

    string(SHA256 _output_key "${_output}")
    get_property(_previous_owner GLOBAL PROPERTY "AROS_SDK_TEXT_OUTPUT_${_output_key}")
    if(_previous_owner)
        message(FATAL_ERROR
            "${ST_NAME}: SDK text output ${_output} is already owned by ${_previous_owner}")
    endif()
    set_property(GLOBAL PROPERTY "AROS_SDK_TEXT_OUTPUT_${_output_key}" "${ST_NAME}")

    set(_serialized_operations "${ST_OPERATIONS}")
    add_custom_command(
        OUTPUT "${_output}"
        COMMAND "${CMAKE_COMMAND}"
            "-DINPUT=${_input}"
            "-DOUTPUT=${_output}"
            "-DBINARY_ROOT=${_binary_root}"
            "-DOUTPUT_ROOT=${_developer_lib_root}"
            "-DINPUT_ROOT=${_fetch_destination}"
            "-DOPERATIONS=${_serialized_operations}"
            ${_declaration_arguments}
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSdkTextRule.cmake"
        DEPENDS "${_fetch_stamp}" ${_declaration_dependencies} "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSdkTextRule.cmake"
        COMMENT "Generating SDK text product ${_output}"
        VERBATIM)
    string(SUBSTRING "${_output_key}" 0 16 _output_suffix)
    set(_product_owner "${ST_NAME}--sdk-text-${_output_suffix}")
    if(TARGET "${_product_owner}")
        message(FATAL_ERROR "${ST_NAME}: duplicate SDK text product target ${_product_owner}")
    endif()
    add_custom_target("${_product_owner}" DEPENDS "${_output}")
    add_dependencies("${ST_NAME}" "${_product_owner}")
endfunction()

function(_aros_validate_sdk_text_operation owner operation)
    if(operation MATCHES "^REPLACE_FIRST_PER_LINE[|]([^|]+)[|](.*)$")
        set(_token "${CMAKE_MATCH_1}")
        set(_replacement "${CMAKE_MATCH_2}")
        if(NOT _token MATCHES "^[A-Za-z0-9_@:/{}$ -]+$")
            message(FATAL_ERROR "${owner}: unsafe REPLACE_FIRST_PER_LINE operation")
        endif()
        _aros_validate_sdk_text_replacement("${owner}" "${_replacement}")
    elseif(operation MATCHES "^REPLACE_ALL[|]([^|]+)[|](.*)$")
        set(_token "${CMAKE_MATCH_1}")
        set(_replacement "${CMAKE_MATCH_2}")
        if(NOT _token MATCHES "^[A-Za-z0-9_@:/{}$ -]+$" OR
           _replacement MATCHES "[;|&\\\\\r\n]")
            message(FATAL_ERROR "${owner}: unsafe REPLACE_ALL operation")
        endif()
        _aros_validate_sdk_text_replacement("${owner}" "${_replacement}")
    elseif(operation MATCHES "^DELETE_LINE_PREFIX[|]([A-Za-z0-9_.+-]+)$")
        set(_prefix "${CMAKE_MATCH_1}")
        if(NOT _prefix)
            message(FATAL_ERROR "${owner}: empty DELETE_LINE_PREFIX operation")
        endif()
    elseif(operation MATCHES "^REPLACE_LINE[|]([A-Za-z0-9_+-]+=)[|](.*)$")
        set(_prefix "${CMAKE_MATCH_1}")
        set(_replacement "${CMAKE_MATCH_2}")
        _aros_validate_sdk_text_replacement("${owner}" "${_replacement}")
    else()
        message(FATAL_ERROR "${owner}: unsupported SDK text operation '${operation}'")
    endif()
endfunction()

function(_aros_validate_sdk_text_replacement owner replacement)
    if("${replacement}" MATCHES "[;|&\\\\\r\n]")
        message(FATAL_ERROR "${owner}: unsafe SDK text replacement")
    endif()
    string(REPLACE "\${prefix}" "" _without_prefix "${replacement}")
    if(_without_prefix MATCHES "[$]")
        message(FATAL_ERROR "${owner}: unsupported variable in SDK text replacement")
    endif()
endfunction()
