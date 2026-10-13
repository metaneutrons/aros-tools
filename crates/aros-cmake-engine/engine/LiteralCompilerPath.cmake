include_guard(GLOBAL)

# Keep literal compiler paths strict by default. The only exception is a
# manifest-declared chain of filename-only aliases in the explicit release
# toolchain's bin directory.
function(_aros_literal_compiler_check_parent_chain path label)
    cmake_path(GET path PARENT_PATH _cursor)
    while(TRUE)
        if(IS_SYMLINK "${_cursor}")
            message(FATAL_ERROR
                "Literal object: symlink compiler driver path refused: symlink parent in ${label}: ${_cursor}")
        endif()
        if(EXISTS "${_cursor}" AND NOT IS_DIRECTORY "${_cursor}")
            message(FATAL_ERROR
                "Literal object: compiler driver has a non-directory parent: ${_cursor}")
        endif()
        cmake_path(GET _cursor PARENT_PATH _parent)
        if(_parent STREQUAL _cursor)
            break()
        endif()
        set(_cursor "${_parent}")
    endwhile()
endfunction()

function(_aros_literal_compiler_assert_regular path)
    find_program(_aros_literal_compiler_test_program
        NAMES test PATHS /usr/bin /bin NO_DEFAULT_PATH NO_CACHE REQUIRED)
    execute_process(
        COMMAND "${_aros_literal_compiler_test_program}" -f "${path}"
        RESULT_VARIABLE _regular_result)
    if(NOT _regular_result EQUAL 0)
        message(FATAL_ERROR
            "Literal object: compiler driver is not a regular file: ${path}")
    endif()
endfunction()

function(_aros_literal_compiler_assert_executable path)
    _aros_literal_compiler_assert_regular("${path}")
    find_program(_aros_literal_compiler_test_program
        NAMES test PATHS /usr/bin /bin NO_DEFAULT_PATH NO_CACHE REQUIRED)
    execute_process(
        COMMAND "${_aros_literal_compiler_test_program}" -x "${path}"
        RESULT_VARIABLE _executable_result)
    if(NOT _executable_result EQUAL 0)
        message(FATAL_ERROR "Literal object: compiler driver is not executable: ${path}")
    endif()
endfunction()

function(_aros_literal_compiler_read_manifest out_json out_sha manifest)
    file(SIZE "${manifest}" _manifest_size)
    if(_manifest_size GREATER 8388608)
        message(FATAL_ERROR "Literal object: toolchain manifest exceeds the 8 MiB limit")
    endif()
    file(SHA256 "${manifest}" _manifest_sha)
    set(_manifest_property "AROS_LITERAL_COMPILER_MANIFEST_${_manifest_sha}")
    get_property(_manifest_cached GLOBAL PROPERTY "${_manifest_property}" SET)
    if(_manifest_cached)
        get_property(_manifest_json GLOBAL PROPERTY "${_manifest_property}")
    else()
        file(READ "${manifest}" _manifest_json)
        string(SHA256 _read_sha "${_manifest_json}")
        if(NOT _read_sha STREQUAL _manifest_sha)
            message(FATAL_ERROR
                "Literal object: toolchain manifest changed while it was being read")
        endif()
        set_property(GLOBAL PROPERTY "${_manifest_property}" "${_manifest_json}")
    endif()
    set(${out_json} "${_manifest_json}" PARENT_SCOPE)
    set(${out_sha} "${_manifest_sha}" PARENT_SCOPE)
endfunction()

function(_aros_literal_compiler_manifest_entry out_kind out_mode out_target
        out_sha out_size manifest_json manifest_sha relative_path)
    string(SHA256 _path_sha "${relative_path}")
    set(_cache_prefix "AROS_LITERAL_COMPILER_INVENTORY_${manifest_sha}_${_path_sha}")
    get_property(_cache_set GLOBAL PROPERTY "${_cache_prefix}_SET" SET)
    if(_cache_set)
        foreach(_field IN ITEMS KIND MODE TARGET SHA SIZE)
            get_property(_cached_${_field} GLOBAL PROPERTY "${_cache_prefix}_${_field}")
        endforeach()
        set(${out_kind} "${_cached_KIND}" PARENT_SCOPE)
        set(${out_mode} "${_cached_MODE}" PARENT_SCOPE)
        set(${out_target} "${_cached_TARGET}" PARENT_SCOPE)
        set(${out_sha} "${_cached_SHA}" PARENT_SCOPE)
        set(${out_size} "${_cached_SIZE}" PARENT_SCOPE)
        return()
    endif()

    string(JSON _files_type ERROR_VARIABLE _json_error TYPE "${manifest_json}" files)
    if(NOT _json_error STREQUAL "NOTFOUND" OR NOT _files_type STREQUAL "ARRAY")
        message(FATAL_ERROR "Literal object: toolchain manifest files inventory is not an array")
    endif()
    string(JSON _files_length LENGTH "${manifest_json}" files)
    if(_files_length GREATER 65536)
        message(FATAL_ERROR "Literal object: toolchain manifest exceeds the 65,536-entry limit")
    endif()
    set(_matches 0)
    foreach(_index RANGE 0 ${_files_length})
        if(_index GREATER_EQUAL _files_length)
            break()
        endif()
        string(JSON _entry_json ERROR_VARIABLE _entry_error GET
            "${manifest_json}" files ${_index})
        if(NOT _entry_error STREQUAL "NOTFOUND")
            message(FATAL_ERROR "Literal object: toolchain manifest inventory entry is invalid")
        endif()
        string(JSON _entry_type ERROR_VARIABLE _type_error TYPE "${_entry_json}")
        if(NOT _type_error STREQUAL "NOTFOUND" OR NOT _entry_type STREQUAL "OBJECT")
            message(FATAL_ERROR "Literal object: toolchain manifest inventory entry is not an object")
        endif()
        string(JSON _path_type ERROR_VARIABLE _path_type_error TYPE "${_entry_json}" path)
        if(NOT _path_type_error STREQUAL "NOTFOUND" OR NOT _path_type STREQUAL "STRING")
            message(FATAL_ERROR "Literal object: toolchain manifest inventory path is invalid")
        endif()
        string(JSON _entry_path GET "${_entry_json}" path)
        if(NOT _entry_path STREQUAL relative_path)
            continue()
        endif()

        math(EXPR _matches "${_matches} + 1")
        if(_matches GREATER 1)
            message(FATAL_ERROR
                "Literal object: toolchain manifest inventories '${relative_path}' more than once")
        endif()
        foreach(_field IN ITEMS type mode)
            string(JSON _field_type ERROR_VARIABLE _field_type_error
                TYPE "${_entry_json}" "${_field}")
            if(NOT _field_type_error STREQUAL "NOTFOUND" OR
               NOT _field_type STREQUAL "STRING")
                message(FATAL_ERROR
                    "Literal object: toolchain manifest '${relative_path}' has invalid ${_field}")
            endif()
            string(JSON _${_field} GET "${_entry_json}" "${_field}")
        endforeach()
        set(_target "")
        set(_sha "")
        set(_size "")
        if(_type STREQUAL "symlink")
            string(JSON _target_type ERROR_VARIABLE _target_error
                TYPE "${_entry_json}" target)
            if(NOT _target_error STREQUAL "NOTFOUND" OR
               NOT _target_type STREQUAL "STRING")
                message(FATAL_ERROR
                    "Literal object: toolchain manifest symlink '${relative_path}' has no target")
            endif()
            string(JSON _target GET "${_entry_json}" target)
        elseif(_type STREQUAL "file")
            string(JSON _sha_type ERROR_VARIABLE _sha_error
                TYPE "${_entry_json}" sha256)
            string(JSON _size_type ERROR_VARIABLE _size_error
                TYPE "${_entry_json}" size)
            if(NOT _sha_error STREQUAL "NOTFOUND" OR
               NOT _sha_type STREQUAL "STRING" OR
               NOT _size_error STREQUAL "NOTFOUND" OR
               NOT _size_type STREQUAL "NUMBER")
                message(FATAL_ERROR
                    "Literal object: toolchain manifest file '${relative_path}' lacks hash/size")
            endif()
            string(JSON _sha GET "${_entry_json}" sha256)
            string(JSON _size GET "${_entry_json}" size)
        else()
            message(FATAL_ERROR
                "Literal object: toolchain manifest '${relative_path}' is not a file or symlink")
        endif()
    endforeach()
    if(NOT _matches EQUAL 1)
        message(FATAL_ERROR
            "Literal object: toolchain manifest does not inventory '${relative_path}' exactly once")
    endif()
    set(${out_kind} "${_type}" PARENT_SCOPE)
    set(${out_mode} "${_mode}" PARENT_SCOPE)
    set(${out_target} "${_target}" PARENT_SCOPE)
    set(${out_sha} "${_sha}" PARENT_SCOPE)
    set(${out_size} "${_size}" PARENT_SCOPE)
    set_property(GLOBAL PROPERTY "${_cache_prefix}_KIND" "${_type}")
    set_property(GLOBAL PROPERTY "${_cache_prefix}_MODE" "${_mode}")
    set_property(GLOBAL PROPERTY "${_cache_prefix}_TARGET" "${_target}")
    set_property(GLOBAL PROPERTY "${_cache_prefix}_SHA" "${_sha}")
    set_property(GLOBAL PROPERTY "${_cache_prefix}_SIZE" "${_size}")
    set_property(GLOBAL PROPERTY "${_cache_prefix}_SET" TRUE)
endfunction()

function(_aros_literal_resolve_compiler out_path out_sha driver)
    if(NOT IS_ABSOLUTE "${driver}")
        message(FATAL_ERROR "Literal object: compiler driver must be an absolute regular file")
    endif()
    cmake_path(ABSOLUTE_PATH driver NORMALIZE OUTPUT_VARIABLE _driver)

    if(NOT IS_SYMLINK "${_driver}")
        _aros_literal_compiler_check_parent_chain("${_driver}" "compiler driver")
        _aros_literal_compiler_assert_executable("${_driver}")
        file(REAL_PATH "${_driver}" _real_driver)
        if(NOT _real_driver STREQUAL _driver)
            message(FATAL_ERROR
                "Literal object: compiler driver path is not canonical: ${_driver}")
        endif()
        file(SHA256 "${_real_driver}" _driver_sha)
        set(${out_path} "${_real_driver}" PARENT_SCOPE)
        set(${out_sha} "${_driver_sha}" PARENT_SCOPE)
        return()
    endif()

    if(NOT DEFINED AROS_CROSS_TOOLCHAIN_ROOT OR
       "${AROS_CROSS_TOOLCHAIN_ROOT}" STREQUAL "" OR
       NOT IS_ABSOLUTE "${AROS_CROSS_TOOLCHAIN_ROOT}")
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: aliases require an explicit AROS_CROSS_TOOLCHAIN_ROOT")
    endif()
    cmake_path(ABSOLUTE_PATH AROS_CROSS_TOOLCHAIN_ROOT NORMALIZE
        OUTPUT_VARIABLE _toolchain_root)
    _aros_literal_compiler_check_parent_chain("${_toolchain_root}/root-check"
        "toolchain root")
    if(IS_SYMLINK "${_toolchain_root}" OR NOT IS_DIRECTORY "${_toolchain_root}")
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: invalid toolchain root")
    endif()
    file(REAL_PATH "${_toolchain_root}" _real_toolchain_root)
    if(NOT _real_toolchain_root STREQUAL _toolchain_root)
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: toolchain root is not canonical")
    endif()
    set(_bin "${_toolchain_root}/bin")
    if(NOT IS_DIRECTORY "${_bin}" OR IS_SYMLINK "${_bin}")
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: toolchain bin is not a regular directory")
    endif()
    file(REAL_PATH "${_bin}" _real_bin)
    if(NOT _real_bin STREQUAL _bin)
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: toolchain bin is not canonical")
    endif()
    cmake_path(GET _driver PARENT_PATH _driver_parent)
    if(NOT _driver_parent STREQUAL _bin)
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: aliases are allowed only directly in toolchain bin")
    endif()
    _aros_literal_compiler_check_parent_chain("${_driver}" "compiler driver")

    set(_manifest "${_toolchain_root}/toolchain-manifest.json")
    if(IS_SYMLINK "${_manifest}" OR NOT EXISTS "${_manifest}" OR
       IS_DIRECTORY "${_manifest}")
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: toolchain manifest is missing or linked")
    endif()
    _aros_literal_compiler_assert_regular("${_manifest}")
    _aros_literal_compiler_read_manifest(_manifest_json _manifest_sha "${_manifest}")

    set(_current "${_driver}")
    set(_seen)
    set(_hop_count 0)
    while(IS_SYMLINK "${_current}")
        if(_hop_count GREATER_EQUAL 8)
            message(FATAL_ERROR
                "Literal object: symlink compiler driver path refused: alias chain exceeds eight links")
        endif()
        cmake_path(GET _current FILENAME _filename)
        if(NOT _filename MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*$" OR
           _filename STREQUAL "." OR _filename STREQUAL "..")
            message(FATAL_ERROR
                "Literal object: symlink compiler driver path refused: unsafe alias filename")
        endif()
        if(_filename IN_LIST _seen)
            message(FATAL_ERROR
                "Literal object: symlink compiler driver path refused: alias cycle detected")
        endif()
        list(APPEND _seen "${_filename}")

        file(READ_SYMLINK "${_current}" _actual_target)
        if(NOT _actual_target MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*$" OR
           _actual_target STREQUAL "." OR _actual_target STREQUAL "..")
            message(FATAL_ERROR
                "Literal object: symlink compiler driver path refused: alias target must be a same-bin filename")
        endif()
        _aros_literal_compiler_manifest_entry(_kind _mode _target _sha _size
            "${_manifest_json}" "${_manifest_sha}" "bin/${_filename}")
        if(NOT _kind STREQUAL "symlink" OR NOT _mode STREQUAL "0777" OR
           NOT _target STREQUAL _actual_target)
            message(FATAL_ERROR
                "Literal object: compiler alias '${_filename}' differs from its toolchain manifest inventory")
        endif()

        set(_current "${_bin}/${_actual_target}")
        if(NOT EXISTS "${_current}" AND NOT IS_SYMLINK "${_current}")
            message(FATAL_ERROR
                "Literal object: symlink compiler driver path refused: alias target is missing")
        endif()
        math(EXPR _hop_count "${_hop_count} + 1")
    endwhile()

    _aros_literal_compiler_check_parent_chain("${_current}" "compiler target")
    if(IS_SYMLINK "${_current}")
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: final compiler remains linked")
    endif()
    _aros_literal_compiler_assert_executable("${_current}")
    file(REAL_PATH "${_current}" _real_compiler)
    if(NOT _real_compiler STREQUAL _current)
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: final compiler path is not canonical")
    endif()
    cmake_path(GET _real_compiler PARENT_PATH _compiler_parent)
    if(NOT _compiler_parent STREQUAL _bin)
        message(FATAL_ERROR
            "Literal object: symlink compiler driver path refused: final compiler escaped toolchain bin")
    endif()
    cmake_path(GET _real_compiler FILENAME _compiler_filename)
    _aros_literal_compiler_manifest_entry(_kind _mode _target _expected_sha _expected_size
        "${_manifest_json}" "${_manifest_sha}" "bin/${_compiler_filename}")
    string(LENGTH "${_expected_sha}" _expected_sha_length)
    if(NOT _kind STREQUAL "file" OR NOT _mode STREQUAL "0755" OR
       NOT _expected_sha_length EQUAL 64 OR
       NOT _expected_sha MATCHES "^[0-9a-f]+$" OR
       NOT _expected_size MATCHES "^[0-9]+$")
        message(FATAL_ERROR
            "Literal object: final compiler is not a manifest-declared executable file")
    endif()
    file(SHA256 "${_real_compiler}" _actual_sha)
    file(SIZE "${_real_compiler}" _actual_size)
    if(NOT _actual_sha STREQUAL _expected_sha OR
       NOT _actual_size STREQUAL _expected_size)
        message(FATAL_ERROR
            "Literal object: final compiler bytes differ from toolchain manifest inventory")
    endif()

    set(${out_path} "${_real_compiler}" PARENT_SCOPE)
    set(${out_sha} "${_actual_sha}" PARENT_SCOPE)
endfunction()
