# GNU RISC-V release-toolchain contract.
#
# This file is selected by AROS_TOOLCHAIN before the legacy LLVM CPU map. The
# CLI has already verified the complete tree; this consumer independently binds
# the executable layout to the manifest and refuses any role path that does not
# resolve to an inventoried file inside the prefix.

function(_aros_gnu_require_members json label expected_fields)
    string(JSON _length ERROR_VARIABLE _json_error LENGTH "${json}")
    if(NOT _json_error STREQUAL "NOTFOUND")
        message(FATAL_ERROR "GNU toolchain ${label} is not a JSON object: ${_json_error}")
    endif()
    set(_expected ${expected_fields})
    list(LENGTH _expected _expected_length)
    if(NOT _length EQUAL _expected_length)
        message(FATAL_ERROR
            "GNU toolchain ${label} has ${_length} fields; expected ${_expected_length}")
    endif()
    set(_seen "")
    math(EXPR _last "${_length} - 1")
    foreach(_index RANGE 0 ${_last})
        string(JSON _member MEMBER "${json}" ${_index})
        if(NOT _member IN_LIST _expected)
            message(FATAL_ERROR "GNU toolchain ${label} has unknown field '${_member}'")
        endif()
        if(_member IN_LIST _seen)
            message(FATAL_ERROR "GNU toolchain ${label} repeats field '${_member}'")
        endif()
        list(APPEND _seen "${_member}")
    endforeach()
    foreach(_member IN LISTS _expected)
        if(NOT _member IN_LIST _seen)
            message(FATAL_ERROR "GNU toolchain ${label} omits field '${_member}'")
        endif()
    endforeach()
endfunction()

function(_aros_gnu_require_type json label member expected_type)
    string(JSON _actual_type ERROR_VARIABLE _json_error TYPE "${json}" "${member}")
    if(NOT _json_error STREQUAL "NOTFOUND" OR
       NOT _actual_type STREQUAL expected_type)
        message(FATAL_ERROR
            "GNU toolchain ${label}.${member} must be ${expected_type}")
    endif()
endfunction()

function(_aros_gnu_json_string out json label member)
    _aros_gnu_require_type("${json}" "${label}" "${member}" STRING)
    string(JSON _value GET "${json}" "${member}")
    set(${out} "${_value}" PARENT_SCOPE)
endfunction()

function(_aros_gnu_validate_token value label)
    if("${value}" STREQUAL "" OR
       NOT "${value}" MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*$")
        message(FATAL_ERROR "GNU toolchain ${label} is empty or unsafe: '${value}'")
    endif()
endfunction()

function(_aros_gnu_validate_sha256 value label)
    string(LENGTH "${value}" _length)
    if(NOT _length EQUAL 64 OR NOT "${value}" MATCHES "^[0-9a-f]+$")
        message(FATAL_ERROR "GNU toolchain ${label} is not a lowercase SHA-256 digest")
    endif()
endfunction()

function(_aros_gnu_validate_version value label minimum maximum)
    if(NOT "${value}" MATCHES "^[0-9]+(\\.[0-9]+)*$")
        message(FATAL_ERROR "GNU toolchain ${label} is not a numeric version")
    endif()
    string(REPLACE "." ";" _components "${value}")
    list(LENGTH _components _component_count)
    if(_component_count LESS minimum OR _component_count GREATER maximum)
        message(FATAL_ERROR "GNU toolchain ${label} has an unsupported component count")
    endif()
    foreach(_component IN LISTS _components)
        string(LENGTH "${_component}" _component_length)
        if(_component_length GREATER 10 OR _component GREATER 2147483647)
            message(FATAL_ERROR "GNU toolchain ${label} has an invalid version component")
        endif()
    endforeach()
endfunction()

function(_aros_gnu_validate_relative_tool_path value label)
    string(LENGTH "${value}" _length)
    if(_length LESS 1 OR _length GREATER 1024 OR
       NOT "${value}" MATCHES "^[A-Za-z0-9._+-]+(/[A-Za-z0-9._+-]+)*$")
        message(FATAL_ERROR "GNU toolchain ${label} is not a portable relative path")
    endif()
    string(REPLACE "/" ";" _segments "${value}")
    list(LENGTH _segments _segment_count)
    if(_segment_count GREATER 32)
        message(FATAL_ERROR "GNU toolchain ${label} exceeds 32 path segments")
    endif()
    foreach(_segment IN LISTS _segments)
        if(_segment STREQUAL "." OR _segment STREQUAL ".." OR
           _segment MATCHES "^-")
            message(FATAL_ERROR "GNU toolchain ${label} contains an unsafe path segment")
        endif()
    endforeach()
endfunction()

function(_aros_gnu_validate_manifest_path value)
    if("${value}" STREQUAL "" OR
       "${value}" MATCHES "^/|/$|\\\\|[\r\n]|//|(^|/)([.]|[.][.])(/|$)|^[A-Za-z]:")
        message(FATAL_ERROR "GNU toolchain manifest inventory has an unsafe path")
    endif()
endfunction()

function(_aros_gnu_inventory_property_name out_name digest relative_path)
    string(SHA256 _path_digest "${relative_path}")
    set(${out_name} "AROS_GNU_INVENTORY_${digest}_${_path_digest}" PARENT_SCOPE)
endfunction()

function(_aros_gnu_index_inventory out_digest manifest_json)
    # Index once per exact manifest byte string. CMake's string(JSON) API
    # reparses its input on each call, so lookups must not rescan files[] for
    # each of the executable and runtime paths.
    string(SHA256 _digest "${manifest_json}")
    set(_index_marker "AROS_GNU_INVENTORY_INDEXED_${_digest}")
    get_property(_already_indexed GLOBAL PROPERTY "${_index_marker}" SET)
    if(_already_indexed)
        set(${out_digest} "${_digest}" PARENT_SCOPE)
        return()
    endif()

    string(JSON _files_type ERROR_VARIABLE _json_error TYPE "${manifest_json}" files)
    if(NOT _json_error STREQUAL "NOTFOUND" OR NOT _files_type STREQUAL "ARRAY")
        message(FATAL_ERROR "GNU toolchain manifest files inventory is not an array")
    endif()
    string(JSON _files_length LENGTH "${manifest_json}" files)
    if(_files_length LESS 1)
        message(FATAL_ERROR "GNU toolchain manifest files inventory is empty")
    endif()

    foreach(_index RANGE 0 ${_files_length})
        if(_index GREATER_EQUAL _files_length)
            break()
        endif()
        string(JSON _entry_json ERROR_VARIABLE _entry_error GET
            "${manifest_json}" files ${_index})
        if(NOT _entry_error STREQUAL "NOTFOUND")
            message(FATAL_ERROR "GNU toolchain manifest inventory entry is invalid")
        endif()

        _aros_gnu_json_string(_entry_path "${_entry_json}" "manifest inventory entry" path)
        _aros_gnu_json_string(_entry_mode "${_entry_json}" "manifest inventory entry" mode)
        _aros_gnu_json_string(_entry_kind "${_entry_json}" "manifest inventory entry" type)
        _aros_gnu_validate_manifest_path("${_entry_path}")
        if(_index GREATER 0)
            if(NOT _previous_path STRLESS "${_entry_path}")
                message(FATAL_ERROR
                    "GNU toolchain manifest inventory paths are duplicated or unsorted")
            endif()
        endif()
        set(_previous_path "${_entry_path}")

        if(_entry_kind STREQUAL "file")
            _aros_gnu_require_members("${_entry_json}" "manifest file entry"
                "path;mode;type;sha256;size")
            _aros_gnu_json_string(_entry_sha "${_entry_json}" "manifest file entry" sha256)
            _aros_gnu_validate_sha256("${_entry_sha}" "inventory sha256")
            _aros_gnu_require_type("${_entry_json}" "manifest file entry" size NUMBER)
            string(JSON _entry_size GET "${_entry_json}" size)
            if(NOT _entry_size MATCHES "^[0-9]+$")
                message(FATAL_ERROR "GNU toolchain inventory file size is not an unsigned integer")
            endif()
            if(NOT _entry_mode MATCHES "^(0644|0755)$")
                message(FATAL_ERROR "GNU toolchain inventory file has an invalid mode")
            endif()
        elseif(_entry_kind STREQUAL "symlink")
            _aros_gnu_require_members("${_entry_json}" "manifest symlink entry"
                "path;mode;type;target")
            _aros_gnu_json_string(_entry_target "${_entry_json}"
                "manifest symlink entry" target)
            if(NOT _entry_mode STREQUAL "0777" OR _entry_target STREQUAL "")
                message(FATAL_ERROR "GNU toolchain inventory symlink has invalid mode or target")
            endif()
        elseif(_entry_kind STREQUAL "directory")
            _aros_gnu_require_members("${_entry_json}" "manifest directory entry"
                "path;mode;type")
            if(NOT _entry_mode STREQUAL "0755")
                message(FATAL_ERROR "GNU toolchain inventory directory has an invalid mode")
            endif()
        else()
            message(FATAL_ERROR "GNU toolchain manifest inventory has unknown type '${_entry_kind}'")
        endif()

        _aros_gnu_inventory_property_name(_entry_property "${_digest}" "${_entry_path}")
        get_property(_entry_already_set GLOBAL PROPERTY "${_entry_property}" SET)
        if(_entry_already_set)
            message(FATAL_ERROR
                "GNU toolchain manifest inventories '${_entry_path}' more than once")
        endif()
        set_property(GLOBAL PROPERTY "${_entry_property}" "${_entry_json}")
    endforeach()
    set_property(GLOBAL PROPERTY "${_index_marker}" TRUE)
    set(${out_digest} "${_digest}" PARENT_SCOPE)
endfunction()

function(_aros_gnu_find_inventory_entry out_kind out_mode out_sha out_size out_target
        manifest_digest relative_path)
    _aros_gnu_inventory_property_name(_entry_property "${manifest_digest}" "${relative_path}")
    get_property(_entry_set GLOBAL PROPERTY "${_entry_property}" SET)
    if(NOT _entry_set)
        message(FATAL_ERROR
            "GNU toolchain manifest does not inventory '${relative_path}' exactly once")
    endif()
    get_property(_entry_json GLOBAL PROPERTY "${_entry_property}")
    _aros_gnu_json_string(_kind "${_entry_json}" "manifest inventory entry" type)
    _aros_gnu_json_string(_mode "${_entry_json}" "manifest inventory entry" mode)
    set(_sha "")
    set(_size "")
    set(_target "")
    if(_kind STREQUAL "file")
        _aros_gnu_json_string(_sha "${_entry_json}" "manifest file entry" sha256)
        string(JSON _size GET "${_entry_json}" size)
    elseif(_kind STREQUAL "symlink")
        _aros_gnu_json_string(_target "${_entry_json}" "manifest symlink entry" target)
    endif()
    set(${out_kind} "${_kind}" PARENT_SCOPE)
    set(${out_mode} "${_mode}" PARENT_SCOPE)
    set(${out_sha} "${_sha}" PARENT_SCOPE)
    set(${out_size} "${_size}" PARENT_SCOPE)
    set(${out_target} "${_target}" PARENT_SCOPE)
endfunction()

function(_aros_gnu_require_inside root_real candidate label out_real)
    if(NOT EXISTS "${candidate}" OR IS_DIRECTORY "${candidate}")
        message(FATAL_ERROR "GNU toolchain ${label} is missing or not a file: ${candidate}")
    endif()
    file(REAL_PATH "${candidate}" _real)
    cmake_path(IS_PREFIX root_real "${_real}" NORMALIZE _inside)
    if(NOT _inside)
        message(FATAL_ERROR "GNU toolchain ${label} resolves outside its prefix: ${candidate}")
    endif()
    set(${out_real} "${_real}" PARENT_SCOPE)
endfunction()

function(_aros_gnu_verify_inventory_file root_real manifest_digest relative_path
        expected_kind expected_mode out_real)
    _aros_gnu_validate_relative_tool_path("${relative_path}" "inventory path")
    _aros_gnu_find_inventory_entry(_kind _mode _sha _size _link_target
        "${manifest_digest}" "${relative_path}")
    if(NOT _kind STREQUAL expected_kind OR NOT _mode STREQUAL expected_mode)
        message(FATAL_ERROR
            "GNU toolchain inventory entry '${relative_path}' has unexpected type or mode")
    endif()
    set(_path "${root_real}/${relative_path}")
    if(IS_SYMLINK "${_path}")
        message(FATAL_ERROR
            "GNU toolchain inventory marks '${relative_path}' as a regular file, but it is linked")
    endif()
    _aros_gnu_require_inside("${root_real}" "${_path}" "${relative_path}" _real)
    if(NOT _kind STREQUAL "file")
        message(FATAL_ERROR "GNU toolchain '${relative_path}' must be a regular file")
    endif()
    string(LENGTH "${_sha}" _sha_length)
    if(NOT _sha_length EQUAL 64 OR NOT _sha MATCHES "^[0-9a-f]+$" OR
       NOT _size MATCHES "^[0-9]+$")
        message(FATAL_ERROR "GNU toolchain inventory entry '${relative_path}' lacks hash/size")
    endif()
    file(SHA256 "${_real}" _actual_sha)
    file(SIZE "${_real}" _actual_size)
    if(NOT _actual_sha STREQUAL _sha OR NOT _actual_size STREQUAL _size)
        message(FATAL_ERROR "GNU toolchain bytes differ from inventory entry '${relative_path}'")
    endif()
    set(${out_real} "${_real}" PARENT_SCOPE)
endfunction()

function(_aros_gnu_resolve_role root_real manifest_digest relative_path role out_path)
    _aros_gnu_validate_relative_tool_path("${relative_path}" "${role} role")
    set(_declared "${root_real}/${relative_path}")
    _aros_gnu_require_inside("${root_real}" "${_declared}" "${role} role" _real)
    _aros_gnu_find_inventory_entry(_kind _mode _sha _size _link_target
        "${manifest_digest}" "${relative_path}")
    if(_kind STREQUAL "file")
        if(NOT _mode STREQUAL "0755")
            message(FATAL_ERROR "GNU toolchain ${role} role is not declared executable")
        endif()
        _aros_gnu_verify_inventory_file("${root_real}" "${manifest_digest}"
            "${relative_path}" file 0755 _verified)
    elseif(_kind STREQUAL "symlink")
        if(NOT _mode STREQUAL "0777" OR _link_target STREQUAL "")
            message(FATAL_ERROR "GNU toolchain ${role} symlink inventory is invalid")
        endif()
        file(READ_SYMLINK "${_declared}" _actual_target)
        if(NOT _actual_target STREQUAL _link_target)
            message(FATAL_ERROR "GNU toolchain ${role} symlink differs from its inventory")
        endif()
        cmake_path(RELATIVE_PATH _real BASE_DIRECTORY "${root_real}"
            OUTPUT_VARIABLE _target_relative)
        _aros_gnu_validate_relative_tool_path("${_target_relative}" "${role} symlink target")
        _aros_gnu_verify_inventory_file("${root_real}" "${manifest_digest}"
            "${_target_relative}" file 0755 _verified)
    else()
        message(FATAL_ERROR "GNU toolchain ${role} role is not an inventoried file")
    endif()
    set(${out_path} "${_declared}" PARENT_SCOPE)
endfunction()

function(_aros_gnu_query_runtime out_path compiler root_real manifest_digest label)
    set(_flags ${AROS_GNU_TARGET_COMPILE_OPTIONS})
    execute_process(
        COMMAND "${compiler}" ${_flags} ${ARGN}
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _output
        ERROR_VARIABLE _error
        OUTPUT_STRIP_TRAILING_WHITESPACE
        TIMEOUT 15)
    if(NOT _result EQUAL 0 OR _output STREQUAL "" OR
       _output MATCHES "[\r\n;]")
        message(FATAL_ERROR
            "GNU toolchain ${label} query failed (${_result}): ${_error}")
    endif()
    if(NOT IS_ABSOLUTE "${_output}")
        message(FATAL_ERROR
            "GNU toolchain ${label} query did not return an absolute prefix path: ${_output}")
    endif()
    _aros_gnu_require_inside("${root_real}" "${_output}" "${label}" _real)
    cmake_path(RELATIVE_PATH _real BASE_DIRECTORY "${root_real}"
        OUTPUT_VARIABLE _runtime_relative)
    _aros_gnu_validate_relative_tool_path("${_runtime_relative}" "${label} path")
    _aros_gnu_find_inventory_entry(_runtime_kind _runtime_mode _runtime_sha
        _runtime_size _runtime_target "${manifest_digest}" "${_runtime_relative}")
    if(NOT _runtime_kind STREQUAL "file" OR
       NOT _runtime_mode MATCHES "^(0644|0755)$")
        message(FATAL_ERROR "GNU toolchain ${label} is not an inventoried runtime file")
    endif()
    _aros_gnu_verify_inventory_file("${root_real}" "${manifest_digest}"
        "${_runtime_relative}" file "${_runtime_mode}" _verified_runtime)
    set(${out_path} "${_real}" PARENT_SCOPE)
endfunction()

if(NOT AROS_CROSS_TOOLCHAIN_ROOT AND DEFINED ENV{AROS_CROSS_TOOLCHAIN_ROOT})
    set(AROS_CROSS_TOOLCHAIN_ROOT "$ENV{AROS_CROSS_TOOLCHAIN_ROOT}")
endif()
if(NOT AROS_CROSS_TOOLCHAIN_ROOT)
    message(FATAL_ERROR "GNU AROS toolchain requires AROS_CROSS_TOOLCHAIN_ROOT")
endif()
if("${AROS_CROSS_TOOLCHAIN_ROOT}" MATCHES "[;\r\n]")
    message(FATAL_ERROR "GNU AROS toolchain root contains a CMake list/control separator")
endif()
if(NOT IS_ABSOLUTE "${AROS_CROSS_TOOLCHAIN_ROOT}" OR
   NOT IS_DIRECTORY "${AROS_CROSS_TOOLCHAIN_ROOT}")
    message(FATAL_ERROR "GNU AROS toolchain root is not an absolute directory")
endif()
file(REAL_PATH "${AROS_CROSS_TOOLCHAIN_ROOT}" _aros_gnu_root)
if(AROS_CROSS_TOOLCHAIN_QUALIFICATION STREQUAL "local-byte-verified" AND
   NOT AROS_CROSS_TOOLCHAIN_ROOT STREQUAL _aros_gnu_root)
    message(FATAL_ERROR "Local GNU compiler root must be canonical and unlinked")
endif()
set(AROS_CROSS_TOOLCHAIN_ROOT "${_aros_gnu_root}" CACHE PATH
    "Immutable GNU AROS toolchain prefix" FORCE)

foreach(_required IN ITEMS AROS_TARGET_CPU AROS_TARGET_PLATFORM
        AROS_TARGET_PROFILE AROS_TARGET_TRIPLE)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "GNU AROS toolchain requires explicit ${_required}")
    endif()
    _aros_gnu_validate_token("${${_required}}" "${_required}")
endforeach()
if(NOT DEFINED AROS_CROSS_TOOLCHAIN_PROFILE)
    # Direct CMake consumers historically use the board profile as the
    # compiler profile. The CLI supplies the measured toolchain profile
    # explicitly when the two namespaces differ.
    set(AROS_CROSS_TOOLCHAIN_PROFILE "${AROS_TARGET_PROFILE}" CACHE STRING
        "Profile expected by the selected GNU compiler prefix")
endif()
_aros_gnu_validate_token("${AROS_CROSS_TOOLCHAIN_PROFILE}"
    "AROS_CROSS_TOOLCHAIN_PROFILE")
if(NOT AROS_TOOLCHAIN STREQUAL "gnu")
    message(FATAL_ERROR "GNU AROS toolchain file requires AROS_TOOLCHAIN=gnu")
endif()
_aros_gnu_validate_token("${AROS_TARGET_TRIPLE}" "target triple")

set(_aros_gnu_manifest "${_aros_gnu_root}/toolchain-manifest.json")
set(_aros_gnu_layout "${_aros_gnu_root}/toolchain-tools.json")
if(AROS_CROSS_TOOLCHAIN_QUALIFICATION STREQUAL "local-byte-verified")
    set(_aros_gnu_identity "${_aros_gnu_root}/toolchain-local.json")
else()
    set(_aros_gnu_identity "${_aros_gnu_manifest}")
endif()
foreach(_contract IN ITEMS "${_aros_gnu_identity}" "${_aros_gnu_layout}")
    if(NOT EXISTS "${_contract}" OR IS_DIRECTORY "${_contract}" OR IS_SYMLINK "${_contract}")
        message(FATAL_ERROR "GNU AROS toolchain contract is missing or linked: ${_contract}")
    endif()
endforeach()
if(AROS_CROSS_TOOLCHAIN_QUALIFICATION STREQUAL "local-byte-verified")
    include("${CMAKE_CURRENT_LIST_DIR}/LocalGnuToolchain.cmake")
    _aros_gnu_load_local_descriptor(_aros_gnu_manifest_json "${_aros_gnu_root}")
    set(_manifest_release_id "")
elseif(NOT AROS_CROSS_TOOLCHAIN_QUALIFICATION STREQUAL "" AND
       DEFINED AROS_CROSS_TOOLCHAIN_QUALIFICATION)
    message(FATAL_ERROR "Unsupported GNU toolchain qualification")
else()
file(READ "${_aros_gnu_manifest}" _aros_gnu_manifest_json)
set(_aros_gnu_manifest_fields
    "schema;release_id;host;target_profile;target_triple;tree_sha256;compiler;recipe_sha256;source_lock_sha256;profiles_sha256;source_commit;producer_commit;tools_commit;source_date_epoch;capabilities;build_environment;files")
_aros_gnu_require_members("${_aros_gnu_manifest_json}" "manifest"
    "${_aros_gnu_manifest_fields}")
_aros_gnu_require_type("${_aros_gnu_manifest_json}" manifest schema NUMBER)
string(JSON _aros_gnu_manifest_schema GET "${_aros_gnu_manifest_json}" schema)
if(NOT _aros_gnu_manifest_schema EQUAL 2)
    message(FATAL_ERROR "GNU AROS toolchain requires manifest schema 2")
endif()
_aros_gnu_json_string(_manifest_release_id "${_aros_gnu_manifest_json}" manifest release_id)
if(_manifest_release_id STREQUAL "")
    message(FATAL_ERROR "GNU AROS toolchain manifest has an empty release_id")
endif()
endif()
foreach(_field IN ITEMS host target_profile target_triple tree_sha256)
    _aros_gnu_json_string(_manifest_${_field} "${_aros_gnu_manifest_json}"
        manifest "${_field}")
    if(_manifest_${_field} STREQUAL "")
        message(FATAL_ERROR "GNU AROS toolchain manifest has an empty ${_field}")
    endif()
endforeach()
if(NOT _manifest_target_profile STREQUAL AROS_CROSS_TOOLCHAIN_PROFILE OR
   NOT _manifest_target_triple STREQUAL AROS_TARGET_TRIPLE)
    message(FATAL_ERROR
        "GNU AROS manifest selects ${_manifest_target_profile}/${_manifest_target_triple}, "
        "requested compiler profile ${AROS_CROSS_TOOLCHAIN_PROFILE} for "
        "board target ${AROS_TARGET_PROFILE}/${AROS_TARGET_TRIPLE}")
endif()
_aros_gnu_validate_sha256("${_manifest_tree_sha256}" "tree_sha256")
_aros_gnu_index_inventory(_aros_gnu_inventory_digest "${_aros_gnu_manifest_json}")
set(AROS_CROSS_TOOLCHAIN_RELEASE_ID "${_manifest_release_id}" CACHE INTERNAL
    "GNU AROS toolchain identity" FORCE)
set(AROS_CROSS_TOOLCHAIN_TREE_SHA256 "${_manifest_tree_sha256}" CACHE INTERNAL
    "GNU AROS toolchain tree digest" FORCE)

# The manifest and executable-layout documents use the same typed compiler
# identity. Compare every field so a valid but differently targeted layout
# cannot redirect any compiler role.
_aros_gnu_require_type("${_aros_gnu_manifest_json}" manifest compiler OBJECT)
string(JSON _aros_gnu_manifest_compiler GET "${_aros_gnu_manifest_json}" compiler)
_aros_gnu_require_members("${_aros_gnu_manifest_compiler}" "manifest compiler"
    "family;gcc_version;binutils_version;target")
file(READ "${_aros_gnu_layout}" _aros_gnu_layout_json)
_aros_gnu_require_members("${_aros_gnu_layout_json}" "toolchain-tools.json"
    "schema;compiler;target_triple;tools")
_aros_gnu_require_type("${_aros_gnu_layout_json}" toolchain-tools schema STRING)
string(JSON _aros_gnu_layout_schema GET "${_aros_gnu_layout_json}" schema)
if(NOT _aros_gnu_layout_schema MATCHES "^aros-toolchain-tools-v[23]$")
    message(FATAL_ERROR "GNU AROS toolchain requires toolchain-tools v2 or v3")
endif()
if(AROS_CROSS_TOOLCHAIN_QUALIFICATION STREQUAL "local-byte-verified" AND
   NOT _aros_gnu_layout_schema STREQUAL "aros-toolchain-tools-v3")
    message(FATAL_ERROR "Local GNU compiler requires toolchain-tools v3")
endif()
_aros_gnu_json_string(_layout_triple "${_aros_gnu_layout_json}" layout target_triple)
if(NOT _layout_triple STREQUAL _manifest_target_triple)
    message(FATAL_ERROR "GNU toolchain layout target triple differs from its manifest")
endif()
_aros_gnu_require_type("${_aros_gnu_layout_json}" layout compiler OBJECT)
string(JSON _aros_gnu_layout_compiler GET "${_aros_gnu_layout_json}" compiler)
_aros_gnu_require_members("${_aros_gnu_layout_compiler}" "layout compiler"
    "family;gcc_version;binutils_version;target")
foreach(_compiler_field IN ITEMS family gcc_version binutils_version)
    _aros_gnu_json_string(_manifest_compiler_${_compiler_field}
        "${_aros_gnu_manifest_compiler}" "manifest compiler" "${_compiler_field}")
    _aros_gnu_json_string(_layout_compiler_${_compiler_field}
        "${_aros_gnu_layout_compiler}" "layout compiler" "${_compiler_field}")
    if(NOT _manifest_compiler_${_compiler_field} STREQUAL
            _layout_compiler_${_compiler_field})
        message(FATAL_ERROR "GNU toolchain compiler ${_compiler_field} differs from its manifest")
    endif()
endforeach()
if(NOT _manifest_compiler_family STREQUAL "gnu")
    message(FATAL_ERROR "GNU AROS manifest has a non-GNU compiler identity")
endif()
_aros_gnu_validate_version("${_manifest_compiler_gcc_version}" "GCC version" 3 3)
_aros_gnu_validate_version("${_manifest_compiler_binutils_version}" "binutils version" 2 4)
_aros_gnu_require_type("${_aros_gnu_manifest_compiler}" "manifest compiler" target OBJECT)
_aros_gnu_require_type("${_aros_gnu_layout_compiler}" "layout compiler" target OBJECT)
string(JSON _manifest_target GET "${_aros_gnu_manifest_compiler}" target)
string(JSON _layout_target GET "${_aros_gnu_layout_compiler}" target)
set(_target_fields
    "schema;isa;abi;code_model;architecture;unaligned_access;atomic_abi;x3_reg_usage")
_aros_gnu_require_members("${_manifest_target}" "manifest RISC-V target" "${_target_fields}")
_aros_gnu_require_members("${_layout_target}" "layout RISC-V target" "${_target_fields}")
foreach(_field IN ITEMS schema isa abi code_model architecture)
    _aros_gnu_json_string(_manifest_target_${_field} "${_manifest_target}"
        "manifest RISC-V target" "${_field}")
    _aros_gnu_json_string(_layout_target_${_field} "${_layout_target}"
        "layout RISC-V target" "${_field}")
    if(NOT _manifest_target_${_field} STREQUAL _layout_target_${_field})
        message(FATAL_ERROR "GNU toolchain target ${_field} differs from its manifest")
    endif()
endforeach()
foreach(_field IN ITEMS unaligned_access atomic_abi x3_reg_usage)
    string(JSON _manifest_target_${_field}_type TYPE "${_manifest_target}" "${_field}")
    string(JSON _layout_target_${_field}_type TYPE "${_layout_target}" "${_field}")
    string(JSON _manifest_target_${_field} GET "${_manifest_target}" "${_field}")
    string(JSON _layout_target_${_field} GET "${_layout_target}" "${_field}")
    if(NOT _manifest_target_${_field}_type STREQUAL _layout_target_${_field}_type OR
       NOT _manifest_target_${_field} STREQUAL _layout_target_${_field})
        message(FATAL_ERROR "GNU toolchain target ${_field} differs from its manifest")
    endif()
endforeach()
if(NOT _manifest_target_schema STREQUAL "aros-riscv-target-v1" OR
   NOT _manifest_target_isa MATCHES "^[a-z0-9_]+$" OR
   NOT _manifest_target_architecture MATCHES "^[a-z0-9_]+$" OR
   NOT _manifest_target_abi MATCHES "^(ilp32|lp64)(f|d)?$" OR
   NOT _manifest_target_code_model MATCHES "^(medlow|medany)$" OR
   NOT _manifest_target_unaligned_access_type STREQUAL "BOOLEAN" OR
   NOT _manifest_target_atomic_abi_type STREQUAL "NUMBER" OR
   NOT _manifest_target_x3_reg_usage_type STREQUAL "NUMBER" OR
   NOT _manifest_target_atomic_abi MATCHES "^[0-3]$" OR
   NOT _manifest_target_x3_reg_usage MATCHES "^[0-3]$")
    message(FATAL_ERROR "GNU AROS manifest has an invalid RISC-V target contract")
endif()
if(_manifest_target_abi MATCHES "^ilp32")
    set(_aros_gnu_expected_cpu "riscv")
    set(_aros_gnu_arch_width "rv32")
    if(NOT _manifest_target_isa MATCHES "^rv32i")
        message(FATAL_ERROR "GNU RISC-V ISA width differs from its ILP32 ABI")
    endif()
    if(NOT AROS_TARGET_CPU STREQUAL "riscv" AND NOT AROS_TARGET_CPU STREQUAL "riscv32")
        message(FATAL_ERROR "GNU RISC-V ILP32 target does not match AROS_TARGET_CPU")
    endif()
else()
    set(_aros_gnu_expected_cpu "riscv64")
    set(_aros_gnu_arch_width "rv64")
    if(NOT (_manifest_target_isa MATCHES "^rv64i" OR
            _manifest_target_isa MATCHES "^rva[a-z0-9_]*u64$"))
        message(FATAL_ERROR "GNU RISC-V ISA width differs from its LP64 ABI")
    endif()
    if(NOT AROS_TARGET_CPU STREQUAL "riscv64")
        message(FATAL_ERROR "GNU RISC-V LP64 target does not match AROS_TARGET_CPU")
    endif()
endif()
if(NOT _manifest_target_architecture MATCHES "^${_aros_gnu_arch_width}i[0-9]+p[0-9]+(_[a-z0-9]+)*$")
    message(FATAL_ERROR "GNU RISC-V architecture attribute disagrees with the target ABI width")
endif()
if(_manifest_target_abi MATCHES "f$" AND
   NOT _manifest_target_architecture MATCHES "(^|_)f[0-9]+p[0-9]+(_|$)")
    message(FATAL_ERROR "GNU RISC-V single-float ABI lacks the F extension")
endif()
if(_manifest_target_abi MATCHES "d$" AND
   (NOT _manifest_target_architecture MATCHES "(^|_)f[0-9]+p[0-9]+(_|$)" OR
    NOT _manifest_target_architecture MATCHES "(^|_)d[0-9]+p[0-9]+(_|$)"))
    message(FATAL_ERROR "GNU RISC-V double-float ABI lacks F/D extensions")
endif()
if(NOT _manifest_target_triple MATCHES "^[A-Za-z0-9_.]+(-[A-Za-z0-9_.]+)*-aros$")
    message(FATAL_ERROR "GNU AROS target triple must end in -aros")
endif()
string(REGEX REPLACE "-aros$" "" _aros_gnu_triple_prefix "${_manifest_target_triple}")
string(REPLACE "-" ";" _aros_gnu_triple_components "${_aros_gnu_triple_prefix}")
foreach(_component IN LISTS _aros_gnu_triple_components)
    if(_component STREQUAL "." OR _component STREQUAL "..")
        message(FATAL_ERROR "GNU AROS target triple contains an unsafe component")
    endif()
endforeach()
string(REGEX REPLACE "-.*$" "" _aros_gnu_triple_cpu "${_aros_gnu_triple_prefix}")
if(NOT _aros_gnu_triple_cpu STREQUAL _aros_gnu_expected_cpu)
    message(FATAL_ERROR "GNU AROS target triple CPU differs from its ABI width")
endif()

_aros_gnu_find_inventory_entry(_layout_kind _layout_mode _layout_sha _layout_size
    _layout_target_link "${_aros_gnu_inventory_digest}" "toolchain-tools.json")
string(LENGTH "${_layout_sha}" _layout_sha_length)
if(NOT _layout_kind STREQUAL "file" OR NOT _layout_sha_length EQUAL 64 OR
   NOT _layout_mode STREQUAL "0644" OR
   NOT _layout_sha MATCHES "^[0-9a-f]+$" OR NOT _layout_size MATCHES "^[0-9]+$")
    message(FATAL_ERROR "GNU AROS manifest does not bind toolchain-tools.json as a file")
endif()
file(SHA256 "${_aros_gnu_layout}" _aros_gnu_layout_sha)
file(SIZE "${_aros_gnu_layout}" _aros_gnu_layout_size)
if(NOT _aros_gnu_layout_sha STREQUAL _layout_sha OR
   NOT _aros_gnu_layout_size STREQUAL _layout_size)
    message(FATAL_ERROR "GNU toolchain-tools.json bytes differ from the manifest inventory")
endif()

_aros_gnu_require_type("${_aros_gnu_layout_json}" layout tools OBJECT)
string(JSON _aros_gnu_tools_json GET "${_aros_gnu_layout_json}" tools)
set(_role_fields "c;cxx;assembler;linker;archive;ranlib;strip;collector;nm;objcopy")
if(_aros_gnu_layout_schema STREQUAL "aros-toolchain-tools-v3")
    list(APPEND _role_fields objdump)
endif()
_aros_gnu_require_members("${_aros_gnu_tools_json}" "GNU executable roles" "${_role_fields}")
foreach(_role IN LISTS _role_fields)
    _aros_gnu_json_string(_role_relative_${_role} "${_aros_gnu_tools_json}"
        "GNU executable roles" "${_role}")
    _aros_gnu_resolve_role("${_aros_gnu_root}" "${_aros_gnu_inventory_digest}"
        "${_role_relative_${_role}}" "${_role}" _role_path_${_role})
endforeach()

# The `assembler` role is raw GNU as. CMake's ASM language needs the GNU C
# driver so .S inputs receive -D/-I preprocessing; the raw assembler remains
# available to engine rules through AROS_AS_BIN.
set(_aros_gnu_driver_asm "${_role_path_c}")
set(_aros_gnu_role_bindings
    "CMAKE_C_COMPILER|${_role_path_c}"
    "CMAKE_CXX_COMPILER|${_role_path_cxx}"
    "CMAKE_ASM_COMPILER|${_aros_gnu_driver_asm}"
    "CMAKE_AR|${_role_path_archive}"
    "CMAKE_RANLIB|${_role_path_ranlib}"
    "CMAKE_STRIP|${_role_path_strip}"
    "CMAKE_NM|${_role_path_nm}"
    "CMAKE_OBJCOPY|${_role_path_objcopy}"
    "AROS_AS_BIN|${_role_path_assembler}"
    "AROS_LINKER_BIN|${_role_path_linker}"
    "AROS_COLLECT_BIN|${_role_path_collector}")
if(_aros_gnu_layout_schema STREQUAL "aros-toolchain-tools-v3")
    list(APPEND _aros_gnu_role_bindings "CMAKE_OBJDUMP|${_role_path_objdump}")
elseif(CMAKE_OBJDUMP)
    # No host or guessed objdump may become a native inspection capability.
    unset(CMAKE_OBJDUMP CACHE)
    unset(CMAKE_OBJDUMP)
endif()
foreach(_binding IN LISTS _aros_gnu_role_bindings)
    string(REPLACE "|" ";" _binding_fields "${_binding}")
    list(GET _binding_fields 0 _variable)
    list(GET _binding_fields 1 _authoritative_path)
    if(DEFINED ${_variable} AND NOT "${${_variable}}" STREQUAL "" AND
       NOT "${${_variable}}" STREQUAL "${_authoritative_path}")
        message(FATAL_ERROR
            "GNU toolchain ${_variable} does not match its inventory-bound role "
            "('${${_variable}}' != '${_authoritative_path}')")
    endif()
    set(${_variable} "${_authoritative_path}" CACHE FILEPATH
        "Inventory-bound GNU AROS tool" FORCE)
endforeach()
set(CMAKE_ASM_COMPILER "${_aros_gnu_driver_asm}" CACHE FILEPATH
    "GNU driver for preprocessed AROS assembly" FORCE)
set(CMAKE_C_COMPILER_TARGET "")
set(CMAKE_CXX_COMPILER_TARGET "")
set(CMAKE_ASM_COMPILER_TARGET "")

# These target switches come from the source-pinned compiler contract and are
# also used for GCC's multilib queries below. No board address or profile-name
# inference participates in compiler selection.
set(AROS_GNU_TARGET_COMPILE_OPTIONS
    "-march=${_manifest_target_isa};-mabi=${_manifest_target_abi};-mcmodel=${_manifest_target_code_model}")
if(_manifest_target_unaligned_access)
    list(APPEND AROS_GNU_TARGET_COMPILE_OPTIONS "-mno-strict-align")
else()
    list(APPEND AROS_GNU_TARGET_COMPILE_OPTIONS "-mstrict-align")
endif()
set(AROS_GNU_TARGET_ARCHITECTURE "${_manifest_target_architecture}" CACHE INTERNAL
    "Source-pinned GNU RISC-V architecture attribute" FORCE)
set(AROS_GNU_TARGET_ATOMIC_ABI "${_manifest_target_atomic_abi}" CACHE INTERNAL
    "Source-pinned GNU RISC-V atomic ABI" FORCE)
set(AROS_GNU_TARGET_X3_REG_USAGE "${_manifest_target_x3_reg_usage}" CACHE INTERNAL
    "Source-pinned GNU RISC-V x3 register contract" FORCE)
set(AROS_GNU_TARGET_COMPILE_OPTIONS "${AROS_GNU_TARGET_COMPILE_OPTIONS}" CACHE INTERNAL
    "Source-pinned GNU RISC-V compiler switches" FORCE)
string(JOIN " " _aros_gnu_flags ${AROS_GNU_TARGET_COMPILE_OPTIONS})
foreach(_language IN ITEMS C CXX ASM)
    string(APPEND CMAKE_${_language}_FLAGS_INIT " ${_aros_gnu_flags}")
endforeach()

_aros_gnu_query_runtime(AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE
    "${_role_path_c}" "${_aros_gnu_root}" "${_aros_gnu_inventory_digest}"
    "libgcc runtime" -print-libgcc-file-name)
_aros_gnu_query_runtime(_aros_gnu_libstdcxx
    "${_role_path_cxx}" "${_aros_gnu_root}" "${_aros_gnu_inventory_digest}"
    "libstdc++ runtime" -print-file-name=libstdc++.a)
_aros_gnu_query_runtime(_aros_gnu_libsupcxx
    "${_role_path_cxx}" "${_aros_gnu_root}" "${_aros_gnu_inventory_digest}"
    "libsupc++ runtime" -print-file-name=libsupc++.a)
set(AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE
    "${AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE}" CACHE FILEPATH
    "Prefix-owned GNU libgcc runtime archive" FORCE)
set(AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES
    "${_aros_gnu_libstdcxx};${_aros_gnu_libsupcxx};${AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE}"
    CACHE INTERNAL "Prefix-owned GNU C++ runtime archives" FORCE)

set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_SYSTEM_PROCESSOR "${AROS_TARGET_CPU}")
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
list(APPEND CMAKE_TRY_COMPILE_PLATFORM_VARIABLES
    AROS_CROSS_TOOLCHAIN_QUALIFICATION AROS_CROSS_TOOLCHAIN_LOCAL_SHA256
    AROS_CROSS_TOOLCHAIN_ROOT AROS_TOOLCHAIN AROS_TARGET_CPU AROS_TARGET_PLATFORM
    AROS_TARGET_PROFILE AROS_CROSS_TOOLCHAIN_PROFILE AROS_TARGET_TRIPLE
    AROS_AS_BIN AROS_LINKER_BIN AROS_COLLECT_BIN
    AROS_GNU_TARGET_COMPILE_OPTIONS AROS_GNU_TARGET_ARCHITECTURE
    AROS_GNU_TARGET_ATOMIC_ABI AROS_GNU_TARGET_X3_REG_USAGE
    AROS_CROSS_TOOLCHAIN_RELEASE_ID AROS_CROSS_TOOLCHAIN_TREE_SHA256
    AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE
    AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES)
list(REMOVE_DUPLICATES CMAKE_TRY_COMPILE_PLATFORM_VARIABLES)
