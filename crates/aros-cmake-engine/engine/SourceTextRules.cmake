cmake_minimum_required(VERSION 3.22)
include_guard(GLOBAL)

# Register one source-proven, literal-only text product. Operations are kept
# in a private JSON file so their contents never pass through CMake's command
# list or shell-escaping layers.
function(aros_transform_source_text)
    set(_one_value_args NAME INPUT OUTPUT FETCH OPERATIONS_JSON MODE)
    cmake_parse_arguments(PARSE_ARGV 0 ST "" "${_one_value_args}" "")
    if(ST_UNPARSED_ARGUMENTS OR ST_KEYWORDS_MISSING_VALUES OR
       NOT ST_NAME OR NOT ST_INPUT OR NOT ST_OUTPUT OR NOT ST_FETCH OR
       NOT ST_OPERATIONS_JSON)
        message(FATAL_ERROR
            "aros_transform_source_text requires NAME, INPUT, OUTPUT, FETCH, "
            "OPERATIONS_JSON and optional MODE")
    endif()
    if(NOT ST_NAME MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*$" OR
       NOT ST_FETCH MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*$")
        message(FATAL_ERROR "source text NAME and FETCH must be safe target names")
    endif()
    if(NOT TARGET "${ST_FETCH}")
        message(FATAL_ERROR "${ST_NAME}: missing source text fetch target ${ST_FETCH}")
    endif()
    if(ST_MODE AND NOT ST_MODE STREQUAL "744")
        message(FATAL_ERROR "${ST_NAME}: source text MODE must be exactly 744 when specified")
    endif()

    foreach(_required_root IN ITEMS AROS_DEVELOPER_INCLUDE_DIR AROS_GENINC_DIR)
        if(NOT DEFINED ${_required_root} OR "${${_required_root}}" STREQUAL "")
            message(FATAL_ERROR "${ST_NAME}: ${_required_root} is not configured")
        endif()
    endforeach()

    find_program(_source_text_test NAMES test PATHS /usr/bin /bin
        NO_DEFAULT_PATH NO_CACHE REQUIRED)
    _aros_source_text_normalize_path("binary root" "${CMAKE_BINARY_DIR}" _binary_root)
    get_property(_fetch_destination TARGET "${ST_FETCH}"
        PROPERTY AROS_FETCH_DESTINATION)
    get_property(_fetch_stamp TARGET "${ST_FETCH}"
        PROPERTY AROS_FETCH_COMPLETION_STAMP)
    if(NOT _fetch_destination OR NOT _fetch_stamp)
        message(FATAL_ERROR
            "${ST_NAME}: fetch target ${ST_FETCH} lacks destination/completion-stamp properties")
    endif()
    _aros_source_text_normalize_path("fetch destination" "${_fetch_destination}" _fetch_destination)
    _aros_source_text_normalize_path("completion stamp" "${_fetch_stamp}" _fetch_stamp)
    _aros_source_text_normalize_path("input" "${ST_INPUT}" _input)
    _aros_source_text_normalize_path("output" "${ST_OUTPUT}" _output)
    _aros_source_text_normalize_path(
        "Developer include root" "${AROS_DEVELOPER_INCLUDE_DIR}" _developer_include_root)
    _aros_source_text_normalize_path("generated include root" "${AROS_GENINC_DIR}" _geninc_root)
    _aros_source_text_normalize_path(
        "source generated include root" "${CMAKE_BINARY_DIR}/gen/include" _source_geninc_root)
    set(_sdk_include_root "")
    if(DEFINED AROS_SDK_INCLUDE_DIR AND NOT "${AROS_SDK_INCLUDE_DIR}" STREQUAL "")
        _aros_source_text_normalize_path(
            "SDK include root" "${AROS_SDK_INCLUDE_DIR}" _sdk_include_root)
    endif()
    _aros_source_text_normalize_path(
        "host tools root" "${CMAKE_BINARY_DIR}/hosttools" _hosttools_root)

    _aros_source_text_require_descendant(
        "fetch destination" "${_fetch_destination}" "${_binary_root}")
    _aros_source_text_require_descendant("completion stamp" "${_fetch_stamp}" "${_binary_root}")
    _aros_source_text_require_descendant("input" "${_input}" "${_fetch_destination}")
    if(_input STREQUAL _fetch_destination)
        message(FATAL_ERROR "${ST_NAME}: input must be a file below the fetch destination")
    endif()
    set(_output_root_variables
        _developer_include_root _geninc_root _source_geninc_root _hosttools_root)
    if(_sdk_include_root)
        list(APPEND _output_root_variables _sdk_include_root)
    endif()
    foreach(_root IN LISTS _output_root_variables)
        _aros_source_text_require_descendant(
            "configured output root" "${${_root}}" "${_binary_root}")
    endforeach()

    set(_selected_output_root "")
    set(_selected_output_kind "")
    foreach(_root_kind IN ITEMS developer sdk generated source-generated hosttools)
        if(_root_kind STREQUAL "developer")
            set(_candidate_root "${_developer_include_root}")
            set(_candidate_group "include")
        elseif(_root_kind STREQUAL "sdk")
            if(NOT _sdk_include_root)
                continue()
            endif()
            set(_candidate_root "${_sdk_include_root}")
            set(_candidate_group "include")
        elseif(_root_kind STREQUAL "generated")
            set(_candidate_root "${_geninc_root}")
            set(_candidate_group "generated")
        elseif(_root_kind STREQUAL "source-generated")
            set(_candidate_root "${_source_geninc_root}")
            set(_candidate_group "generated")
        else()
            set(_candidate_root "${_hosttools_root}")
            set(_candidate_group "hosttools")
        endif()
        cmake_path(IS_PREFIX _candidate_root "${_output}" NORMALIZE _inside_root)
        if(_inside_root AND NOT _output STREQUAL _candidate_root)
            if(_selected_output_root AND
               (NOT _selected_output_root STREQUAL _candidate_root OR
                NOT _selected_output_kind STREQUAL _candidate_group))
                message(FATAL_ERROR
                    "${ST_NAME}: output is ambiguously inside multiple configured roots")
            endif()
            if(NOT _selected_output_root)
                set(_selected_output_root "${_candidate_root}")
                set(_selected_output_kind "${_candidate_group}")
            endif()
        endif()
    endforeach()
    if(NOT _selected_output_root)
        message(FATAL_ERROR
            "${ST_NAME}: output must be below Developer/SDK include, generated include, or hosttools")
    endif()
    if(ST_MODE STREQUAL "744" AND NOT _selected_output_kind STREQUAL "hosttools")
        message(FATAL_ERROR "${ST_NAME}: MODE 744 is allowed only for hosttools output")
    endif()
    if(_input STREQUAL _output)
        message(FATAL_ERROR "${ST_NAME}: source text input and output are identical")
    endif()

    string(LENGTH "${ST_OPERATIONS_JSON}" _recipe_length)
    if(_recipe_length GREATER 1048576)
        message(FATAL_ERROR "${ST_NAME}: source text operations JSON exceeds 1 MiB")
    endif()
    _aros_source_text_validate_operations_json("${ST_NAME}" "${ST_OPERATIONS_JSON}")

    # Inspect every existing component at configure time, while the runner
    # repeats these checks immediately before reading or publishing.
    set(_path_pairs
            "binary root|${_binary_root}"
            "fetch destination|${_fetch_destination}"
            "completion stamp|${_fetch_stamp}"
            "input|${_input}"
            "output|${_output}"
            "Developer include root|${_developer_include_root}"
            "generated include root|${_geninc_root}"
            "source generated include root|${_source_geninc_root}"
            "host tools root|${_hosttools_root}")
    if(_sdk_include_root)
        list(APPEND _path_pairs "SDK include root|${_sdk_include_root}")
    endif()
    foreach(_path_pair IN LISTS _path_pairs)
        string(FIND "${_path_pair}" "|" _separator)
        string(SUBSTRING "${_path_pair}" 0 ${_separator} _label)
        math(EXPR _path_start "${_separator} + 1")
        string(SUBSTRING "${_path_pair}" ${_path_start} -1 _path)
        _aros_source_text_check_components("${_label}" "${_path}" "${_binary_root}")
        _aros_source_text_check_existing_ancestor("${_label}" "${_path}" "${_binary_root}")
    endforeach()
    _aros_source_text_check_directory("binary root" "${_binary_root}" TRUE)
    set(_existing_roots _fetch_destination)
    list(APPEND _existing_roots ${_output_root_variables})
    foreach(_root IN LISTS _existing_roots)
        _aros_source_text_check_directory("configured root" "${${_root}}" FALSE)
    endforeach()
    _aros_source_text_check_directory("output root" "${_selected_output_root}" FALSE)

    _aros_source_text_path_exists("${_input}" _input_exists)
    if(_input_exists)
        _aros_source_text_require_regular_file("input" "${_input}" "${_source_text_test}")
        file(SIZE "${_input}" _input_size)
        if(_input_size GREATER 33554432)
            message(FATAL_ERROR "${ST_NAME}: source text input exceeds 32 MiB")
        endif()
    endif()
    _aros_source_text_path_exists("${_output}" _output_exists)
    if(_output_exists)
        _aros_source_text_require_regular_file("existing output" "${_output}" "${_source_text_test}")
    endif()
    _aros_source_text_path_exists("${_fetch_stamp}" _stamp_exists)
    if(_stamp_exists)
        _aros_source_text_require_regular_file(
            "fetch completion stamp" "${_fetch_stamp}" "${_source_text_test}")
    endif()

    # A global output registry rejects every duplicate claim, including two
    # declarations from the same named aggregate target.
    string(SHA256 _output_key "${_output}")
    get_property(_previous_owner GLOBAL PROPERTY "AROS_SOURCE_TEXT_OUTPUT_${_output_key}")
    if(_previous_owner)
        message(FATAL_ERROR
            "${ST_NAME}: source text output ${_output} is already owned by ${_previous_owner}")
    endif()

    # Repeated NAME values aggregate independent product targets. An unrelated
    # target with the same spelling is never adopted.
    string(SHA256 _aggregate_key "${ST_NAME}")
    get_property(_aggregate_owner GLOBAL PROPERTY
        "AROS_SOURCE_TEXT_AGGREGATE_${_aggregate_key}")
    if(TARGET "${ST_NAME}" AND NOT "${_aggregate_owner}" STREQUAL "${ST_NAME}")
        message(FATAL_ERROR "${ST_NAME}: source text aggregate target is already declared")
    elseif(NOT TARGET "${ST_NAME}")
        add_custom_target("${ST_NAME}")
        set_property(GLOBAL PROPERTY "AROS_SOURCE_TEXT_AGGREGATE_${_aggregate_key}" "${ST_NAME}")
    endif()

    set(_recipe_root "${_binary_root}/.aros-source-text-rules")
    set(_recipe "${_recipe_root}/${_output_key}.json")
    set(_depfile "${_recipe_root}/${_output_key}.d")
    _aros_source_text_check_components(
        "recipe root" "${_recipe_root}" "${_binary_root}")
    _aros_source_text_path_exists("${_recipe}" _recipe_exists)
    if(_recipe_exists)
        _aros_source_text_require_regular_file("existing private recipe" "${_recipe}" "${_source_text_test}")
    endif()
    file(MAKE_DIRECTORY "${_recipe_root}")
    _aros_source_text_check_components(
        "private recipe" "${_recipe}" "${_binary_root}")
    _aros_source_text_check_existing_ancestor("private recipe" "${_recipe}" "${_binary_root}")
    _aros_source_text_check_components(
        "private depfile" "${_depfile}" "${_binary_root}")
    _aros_source_text_path_exists("${_depfile}" _depfile_exists)
    if(_depfile_exists)
        _aros_source_text_require_regular_file("existing private depfile" "${_depfile}" "${_source_text_test}")
    endif()
    file(WRITE "${_recipe}" "${ST_OPERATIONS_JSON}")

    set(_runner "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSourceTextRule.cmake")
    string(SHA256 _product_hash "${_output}")
    string(SUBSTRING "${_product_hash}" 0 16 _product_suffix)
    set(_product_target "${ST_NAME}--source-text-${_product_suffix}")
    if(TARGET "${_product_target}")
        message(FATAL_ERROR "${ST_NAME}: duplicate source text product target ${_product_target}")
    endif()

    set(_command
        "${CMAKE_COMMAND}"
        "-DINPUT=${_input}"
        "-DOUTPUT=${_output}"
        "-DINPUT_ROOT=${_fetch_destination}"
        "-DOUTPUT_ROOT=${_selected_output_root}"
        "-DDEVELOPER_INCLUDE_ROOT=${_developer_include_root}"
        "-DSDK_INCLUDE_ROOT=${_sdk_include_root}"
        "-DGEN_INCLUDE_ROOT=${_geninc_root}"
        "-DSOURCE_GEN_INCLUDE_ROOT=${_source_geninc_root}"
        "-DHOSTTOOLS_ROOT=${_hosttools_root}"
        "-DBINARY_ROOT=${_binary_root}"
        "-DRECIPE=${_recipe}"
        "-DDEPFILE=${_depfile}")
    if(ST_MODE)
        list(APPEND _command "-DMODE=${ST_MODE}")
    endif()
    list(APPEND _command -P "${_runner}")

    set(_dependencies "${_fetch_stamp}" "${_recipe}" "${_runner}")
    if(_input_exists)
        list(APPEND _dependencies "${_input}")
    endif()
    add_custom_command(
        OUTPUT "${_output}" "${_depfile}"
        COMMAND ${_command}
        DEPENDS ${_dependencies}
        DEPFILE "${_depfile}"
        COMMENT "Transforming source text ${_output}"
        VERBATIM)
    add_custom_target("${_product_target}" DEPENDS "${_output}")
    add_dependencies("${_product_target}" "${ST_FETCH}")
    add_dependencies("${ST_NAME}" "${_product_target}" "${ST_FETCH}")
    set_property(GLOBAL PROPERTY "AROS_SOURCE_TEXT_OUTPUT_${_output_key}" "${ST_NAME}")
endfunction()

function(_aros_source_text_normalize_path label raw out)
    if(NOT IS_ABSOLUTE "${raw}" OR "${raw}" MATCHES "[;\"\r\n$`|&]")
        message(FATAL_ERROR "source text ${label} is not a safe absolute path: ${raw}")
    endif()
    string(LENGTH "${raw}" _length)
    if(_length GREATER 4096)
        message(FATAL_ERROR "source text ${label} exceeds the path length limit")
    endif()
    string(REPLACE "/" ";" _components "${raw}")
    foreach(_component IN LISTS _components)
        if(_component STREQUAL "." OR _component STREQUAL "..")
            message(FATAL_ERROR "source text ${label} contains a dot path component: ${raw}")
        endif()
    endforeach()
    set(_candidate "${raw}")
    cmake_path(NORMAL_PATH _candidate OUTPUT_VARIABLE _normalized)
    if(_normalized STREQUAL "/")
        message(FATAL_ERROR "source text ${label} cannot be the filesystem root")
    endif()
    set(${out} "${_normalized}" PARENT_SCOPE)
endfunction()

function(_aros_source_text_require_descendant label path root)
    cmake_path(IS_PREFIX root "${path}" NORMALIZE _inside)
    if(NOT _inside OR "${path}" STREQUAL "${root}")
        message(FATAL_ERROR "source text ${label} escapes its declared root")
    endif()
endfunction()

function(_aros_source_text_check_components label path root)
    if(IS_SYMLINK "${root}")
        message(FATAL_ERROR "source text ${label} starts at a symlinked root: ${root}")
    endif()
    file(RELATIVE_PATH _relative "${root}" "${path}")
    if(_relative MATCHES "(^|/)\\.\\.(/|$)" OR _relative MATCHES "^\\.\\.")
        message(FATAL_ERROR "source text ${label} escapes its checked root")
    endif()
    if(_relative STREQUAL "." OR _relative STREQUAL "")
        return()
    endif()
    string(REPLACE "/" ";" _components "${_relative}")
    set(_probe "${root}")
    list(LENGTH _components _component_count)
    set(_index 0)
    foreach(_component IN LISTS _components)
        if(_component STREQUAL "")
            continue()
        endif()
        math(EXPR _index "${_index} + 1")
        set(_probe "${_probe}/${_component}")
        if(IS_SYMLINK "${_probe}")
            message(FATAL_ERROR "source text ${label} crosses a symlink: ${_probe}")
        endif()
        if(EXISTS "${_probe}" AND _index LESS _component_count AND
           NOT IS_DIRECTORY "${_probe}")
            message(FATAL_ERROR "source text ${label} parent is not a directory: ${_probe}")
        endif()
    endforeach()
endfunction()

function(_aros_source_text_check_existing_ancestor label path binary_root)
    set(_existing "${path}")
    while(NOT EXISTS "${_existing}")
        cmake_path(GET _existing PARENT_PATH _parent)
        if(_parent STREQUAL _existing)
            message(FATAL_ERROR "source text ${label} has no existing safe ancestor")
        endif()
        set(_existing "${_parent}")
    endwhile()
    file(REAL_PATH "${binary_root}" _binary_real)
    file(REAL_PATH "${_existing}" _existing_real)
    cmake_path(IS_PREFIX _binary_real "${_existing_real}" NORMALIZE _inside)
    if(NOT _inside)
        message(FATAL_ERROR "source text ${label} physically escapes the binary directory")
    endif()
endfunction()

function(_aros_source_text_check_directory label path must_exist)
    _aros_source_text_path_exists("${path}" _exists)
    if(_exists AND NOT IS_DIRECTORY "${path}")
        message(FATAL_ERROR "source text ${label} is not a directory: ${path}")
    endif()
    if(must_exist AND NOT IS_DIRECTORY "${path}")
        message(FATAL_ERROR "source text ${label} does not exist: ${path}")
    endif()
endfunction()

function(_aros_source_text_path_exists path out)
    execute_process(
        COMMAND "${_source_text_test}" -e "${path}"
        RESULT_VARIABLE _exists_result
        OUTPUT_QUIET ERROR_QUIET TIMEOUT 10)
    if(_exists_result STREQUAL "0")
        set(${out} TRUE PARENT_SCOPE)
    else()
        set(${out} FALSE PARENT_SCOPE)
    endif()
endfunction()

function(_aros_source_text_require_regular_file label path test_program)
    execute_process(
        COMMAND "${test_program}" -f "${path}"
        RESULT_VARIABLE _regular_result
        OUTPUT_QUIET ERROR_QUIET TIMEOUT 10)
    if(NOT _regular_result STREQUAL "0")
        message(FATAL_ERROR "source text ${label} is not a regular file: ${path}")
    endif()
endfunction()

function(_aros_source_text_validate_operations_json owner json)
    string(JSON _root_type ERROR_VARIABLE _json_error TYPE "${json}")
    if(NOT _json_error STREQUAL "NOTFOUND" OR NOT _root_type STREQUAL "ARRAY")
        message(FATAL_ERROR "${owner}: OPERATIONS_JSON must be a JSON array")
    endif()
    string(JSON _count ERROR_VARIABLE _json_error LENGTH "${json}")
    if(NOT _json_error STREQUAL "NOTFOUND" OR _count LESS 1 OR _count GREATER 128)
        message(FATAL_ERROR "${owner}: operations must contain 1 to 128 entries")
    endif()
    math(EXPR _last_index "${_count} - 1")
    foreach(_index RANGE 0 ${_last_index})
        string(JSON _operation_type ERROR_VARIABLE _json_error TYPE "${json}" ${_index})
        if(NOT _json_error STREQUAL "NOTFOUND" OR NOT _operation_type STREQUAL "OBJECT")
            message(FATAL_ERROR "${owner}: operation ${_index} must be an object")
        endif()
        string(JSON _member_count ERROR_VARIABLE _json_error LENGTH "${json}" ${_index})
        if(NOT _json_error STREQUAL "NOTFOUND" OR NOT _member_count EQUAL 3)
            message(FATAL_ERROR
                "${owner}: operation ${_index} must contain exactly kind, token, and replacement")
        endif()
        foreach(_field IN ITEMS kind token replacement)
            string(JSON _field_type ERROR_VARIABLE _json_error TYPE
                "${json}" ${_index} "${_field}")
            if(NOT _json_error STREQUAL "NOTFOUND" OR NOT _field_type STREQUAL "STRING")
                message(FATAL_ERROR "${owner}: operation ${_index} ${_field} must be a string")
            endif()
        endforeach()
        string(JSON _kind GET "${json}" ${_index} kind)
        string(JSON _token GET "${json}" ${_index} token)
        string(JSON _replacement GET "${json}" ${_index} replacement)
        if(NOT _kind STREQUAL "replace_all" AND
           NOT _kind STREQUAL "replace_whole_line_containing")
            message(FATAL_ERROR "${owner}: operation ${_index} has unsupported kind '${_kind}'")
        endif()
        if(_token STREQUAL "")
            message(FATAL_ERROR "${owner}: operation ${_index} has an empty token")
        endif()
        string(LENGTH "${_token}" _token_length)
        string(LENGTH "${_replacement}" _replacement_length)
        if(_token_length GREATER 262144 OR _replacement_length GREATER 262144)
            message(FATAL_ERROR "${owner}: operation ${_index} token/replacement exceeds 256 KiB")
        endif()
    endforeach()
endfunction()
