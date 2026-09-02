# Bounded traversal of literal includes through staged-header ownership.
#
# A genmodule config can include a checked-in public header which in turn
# includes a header supplied by a fetched port.  The compiler sees that chain
# only after the port header should already exist, so derive the complete owner
# set at configure time from the source provenance recorded by
# aros_copy_includes().  Traversal follows only declared staging bindings, is
# cycle-safe, and has both depth and file-count bounds.

include_guard(GLOBAL)

function(_aros_parse_staged_header_binding
        binding out_header out_owner out_hash out_source)
    string(REPLACE "|" ";" _fields "${binding}")
    list(LENGTH _fields _length)
    if(_length LESS 2 OR _length GREATER 4)
        set(${out_header} "" PARENT_SCOPE)
        set(${out_owner} "" PARENT_SCOPE)
        set(${out_hash} "" PARENT_SCOPE)
        set(${out_source} "" PARENT_SCOPE)
        return()
    endif()
    list(GET _fields 0 _header)
    list(GET _fields 1 _owner)
    set(_hash "")
    set(_source "")
    if(_length GREATER 2)
        list(GET _fields 2 _hash)
    endif()
    if(_length GREATER 3)
        list(GET _fields 3 _source)
    endif()
    set(${out_header} "${_header}" PARENT_SCOPE)
    set(${out_owner} "${_owner}" PARENT_SCOPE)
    set(${out_hash} "${_hash}" PARENT_SCOPE)
    set(${out_source} "${_source}" PARENT_SCOPE)
endfunction()

# Build a constant-time lookup table once every %copy_includes declaration has
# been evaluated. Early genmodule consumers intentionally keep using the plain
# list while the graph is still growing; the tree-wide source pass calls this
# only after generated_targets.cmake is complete.
function(_aros_prepare_staged_header_binding_index)
    get_property(_bindings GLOBAL PROPERTY AROS_STAGED_HEADER_BINDINGS)
    foreach(_binding IN LISTS _bindings)
        _aros_parse_staged_header_binding("${_binding}"
            _header _owner _hash _source)
        if(NOT _header)
            continue()
        endif()
        string(SHA256 _header_key "${_header}")
        set_property(GLOBAL APPEND PROPERTY
            "AROS_STAGED_HEADER_BINDING_INDEX_${_header_key}" "${_binding}")
    endforeach()
    set_property(GLOBAL PROPERTY AROS_STAGED_HEADER_BINDING_INDEX_READY TRUE)
endfunction()

# _aros_collect_transitive_header_bindings(<owners-var> <hashes-var> <file>...)
#
# AROS_STAGED_HEADER_BINDINGS entries have this stable shape:
#   <public header>|<owner target>|<optional deferred hash>|<source file>
# Older two/three-field entries remain accepted for compatibility, but cannot
# be traversed without their source field.
function(_aros_collect_transitive_header_bindings
        out_owners out_deferred_hashes)
    get_property(_bindings GLOBAL PROPERTY AROS_STAGED_HEADER_BINDINGS)
    get_property(_binding_index_ready GLOBAL PROPERTY
        AROS_STAGED_HEADER_BINDING_INDEX_READY)
    if(NOT _bindings OR NOT ARGN)
        set(${out_owners} "" PARENT_SCOPE)
        set(${out_deferred_hashes} "" PARENT_SCOPE)
        return()
    endif()

    set(_max_depth 16)
    set(_max_files 256)
    set(_queue "")
    foreach(_initial_file IN LISTS ARGN)
        if(EXISTS "${_initial_file}")
            list(APPEND _queue "0|${_initial_file}")
        endif()
    endforeach()
    set(_visited "")
    set(_owners "")
    set(_deferred_hashes "")
    set(_scanned 0)

    while(_queue AND _scanned LESS _max_files)
        list(POP_FRONT _queue _entry)
        string(FIND "${_entry}" "|" _separator)
        if(_separator LESS 1)
            continue()
        endif()
        string(SUBSTRING "${_entry}" 0 ${_separator} _depth)
        math(EXPR _path_start "${_separator} + 1")
        string(SUBSTRING "${_entry}" ${_path_start} -1 _path)
        if(_path IN_LIST _visited OR NOT EXISTS "${_path}")
            continue()
        endif()
        list(APPEND _visited "${_path}")
        math(EXPR _scanned "${_scanned} + 1")

        file(STRINGS "${_path}" _include_lines
            REGEX "^[ \t]*#[ \t]*include[ \t]+[<\"]")
        foreach(_line IN LISTS _include_lines)
            if(NOT _line MATCHES
               "^[ \t]*#[ \t]*include[ \t]+[<\"]([^>\"]+)[>\"]")
                continue()
            endif()
            set(_included_header "${CMAKE_MATCH_1}")
            if(_binding_index_ready)
                string(SHA256 _included_header_key "${_included_header}")
                get_property(_matching_bindings GLOBAL PROPERTY
                    "AROS_STAGED_HEADER_BINDING_INDEX_${_included_header_key}")
            else()
                set(_matching_bindings "${_bindings}")
            endif()
            foreach(_binding IN LISTS _matching_bindings)
                _aros_parse_staged_header_binding("${_binding}"
                    _header _owner _hash _source)
                if(NOT _header STREQUAL _included_header OR NOT _owner)
                    continue()
                endif()
                list(APPEND _owners "${_owner}")
                if(_hash)
                    list(APPEND _deferred_hashes "${_hash}")
                endif()
                if(_depth LESS _max_depth AND _source AND EXISTS "${_source}")
                    math(EXPR _next_depth "${_depth} + 1")
                    list(APPEND _queue "${_next_depth}|${_source}")
                endif()
            endforeach()

            # Port sources with an extensionless MetaMake spelling are reached
            # through the small wrapper produced by aros_resolve_sources().
            # Follow literal quoted local includes as well as staged public
            # headers, so that wrapper -> fetched source -> public header is
            # one dependency chain. Angle includes remain confined to declared
            # staged bindings; an arbitrary host/system include must never
            # become configure-time input.
            if(_depth LESS _max_depth AND _line MATCHES
               "^[ \t]*#[ \t]*include[ \t]+\"([^\"]+)\"")
                set(_local_include "${CMAKE_MATCH_1}")
                if(IS_ABSOLUTE "${_local_include}")
                    set(_local_path "${_local_include}")
                else()
                    get_filename_component(_current_dir "${_path}" DIRECTORY)
                    set(_local_path "${_current_dir}/${_local_include}")
                endif()
                cmake_path(NORMAL_PATH _local_path)
                if(EXISTS "${_local_path}")
                    math(EXPR _next_depth "${_depth} + 1")
                    list(APPEND _queue "${_next_depth}|${_local_path}")
                endif()
            endif()
        endforeach()
    endwhile()

    list(REMOVE_DUPLICATES _owners)
    list(REMOVE_DUPLICATES _deferred_hashes)
    set(${out_owners} "${_owners}" PARENT_SCOPE)
    set(${out_deferred_hashes} "${_deferred_hashes}" PARENT_SCOPE)
endfunction()
