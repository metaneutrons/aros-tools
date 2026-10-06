cmake_minimum_required(VERSION 3.22)

include("${CMAKE_CURRENT_LIST_DIR}/../TransitiveHeaderBindings.cmake")

set(_fixture "${CMAKE_CURRENT_LIST_DIR}/transitive-header-bindings")
set_property(GLOBAL PROPERTY AROS_STAGED_HEADER_BINDINGS
    "GL/gla.h|gl-owner||${_fixture}/gla.h"
    "GL/gl.h|mesa-owner|0123456789abcdef|${_fixture}/not-fetched/gl.h")

_aros_collect_transitive_header_bindings(
    _owners _hashes "${_fixture}/root.conf")

if(NOT _owners STREQUAL "gl-owner;mesa-owner")
    message(FATAL_ERROR "unexpected transitive owners: ${_owners}")
endif()
if(NOT _hashes STREQUAL "0123456789abcdef")
    message(FATAL_ERROR "unexpected deferred hashes: ${_hashes}")
endif()

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _wrapper_suffix)
set(_wrapper_temp "$ENV{TMPDIR}")
if(NOT _wrapper_temp)
    set(_wrapper_temp "${CMAKE_CURRENT_BINARY_DIR}")
endif()
set(_wrapper "${_wrapper_temp}/aros-transitive-header-${_wrapper_suffix}.c")
file(WRITE "${_wrapper}"
    "/* generated source wrapper */\n#include \"${_fixture}/gla.h\"\n")
_aros_collect_transitive_header_bindings(
    _wrapper_owners _wrapper_hashes "${_wrapper}")
if(NOT _wrapper_owners STREQUAL "gl-owner;mesa-owner")
    message(FATAL_ERROR
        "quoted local source traversal lost owners: ${_wrapper_owners}")
endif()
if(NOT _wrapper_hashes STREQUAL "0123456789abcdef")
    message(FATAL_ERROR
        "quoted local source traversal lost hashes: ${_wrapper_hashes}")
endif()
file(REMOVE "${_wrapper}")

# Collection happens before the full generated-target graph is available. Keep
# changing the declaration list between calls to prove the lazy index follows
# newly appended and replaced bindings in that early path.
string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _binding_suffix)
set(_binding_temp_root "${_wrapper_temp}/aros-transitive-bindings-${_binding_suffix}")
file(MAKE_DIRECTORY "${_binding_temp_root}/Dynamic")
set(_dynamic_root "${_binding_temp_root}/root.c")
set(_dynamic_trigger "${_binding_temp_root}/Dynamic/trigger.h")
set(_dynamic_changing "${_binding_temp_root}/Dynamic/changing.h")
set(_dynamic_extra "${_binding_temp_root}/Dynamic/extra.h")
file(WRITE "${_dynamic_root}" "#include <Dynamic/trigger.h>\n")
file(WRITE "${_dynamic_trigger}" "#include <Dynamic/changing.h>\n")
file(WRITE "${_dynamic_changing}" "#include <Dynamic/extra.h>\n")
file(WRITE "${_dynamic_extra}" "/* staged extra header */\n")

set(_dynamic_base_bindings
    "Dynamic/trigger.h|trigger-owner||${_dynamic_trigger}"
    "Dynamic/changing.h|old-owner|old-hash|${_dynamic_changing}")
set_property(GLOBAL PROPERTY AROS_STAGED_HEADER_BINDINGS
    "${_dynamic_base_bindings}")
_aros_collect_transitive_header_bindings(
    _dynamic_initial_owners _dynamic_initial_hashes "${_dynamic_root}")
if(NOT _dynamic_initial_owners STREQUAL "trigger-owner;old-owner")
    message(FATAL_ERROR
        "unexpected initial dynamic owners: ${_dynamic_initial_owners}")
endif()
if(NOT _dynamic_initial_hashes STREQUAL "old-hash")
    message(FATAL_ERROR
        "unexpected initial dynamic hashes: ${_dynamic_initial_hashes}")
endif()

set(_dynamic_appended_bindings
    "${_dynamic_base_bindings};Dynamic/extra.h|appended-owner||${_dynamic_extra}")
set_property(GLOBAL PROPERTY AROS_STAGED_HEADER_BINDINGS
    "${_dynamic_appended_bindings}")
_aros_collect_transitive_header_bindings(
    _dynamic_appended_owners _dynamic_appended_hashes "${_dynamic_root}")
if(NOT _dynamic_appended_owners STREQUAL
   "trigger-owner;old-owner;appended-owner")
    message(FATAL_ERROR
        "newly appended binding was missed: ${_dynamic_appended_owners}")
endif()

set(_dynamic_replacement_bindings
    "Dynamic/trigger.h|trigger-owner||${_dynamic_trigger};Dynamic/changing.h|replacement-owner|replacement-hash|${_dynamic_changing};Dynamic/extra.h|appended-owner||${_dynamic_extra}")
set_property(GLOBAL PROPERTY AROS_STAGED_HEADER_BINDINGS
    "${_dynamic_replacement_bindings}")
_aros_collect_transitive_header_bindings(
    _dynamic_replaced_owners _dynamic_replaced_hashes "${_dynamic_root}")
if(NOT _dynamic_replaced_owners STREQUAL
   "trigger-owner;replacement-owner;appended-owner")
    message(FATAL_ERROR
        "replaced binding was not honored: ${_dynamic_replaced_owners}")
endif()
if(NOT _dynamic_replaced_hashes STREQUAL "replacement-hash")
    message(FATAL_ERROR
        "stale deferred hash remained after replacement: ${_dynamic_replaced_hashes}")
endif()

# A same-list prepare is a no-op: each of the three current headers has one
# indexed entry, while buckets belonging to the earlier list are cleared.
_aros_prepare_staged_header_binding_index()
_aros_prepare_staged_header_binding_index()
get_property(_bucket_keys GLOBAL PROPERTY
    AROS_STAGED_HEADER_BINDING_INDEX_BUCKET_KEYS)
list(LENGTH _bucket_keys _bucket_count)
if(NOT _bucket_count EQUAL 3)
    message(FATAL_ERROR "expected three current index buckets, found ${_bucket_count}")
endif()
foreach(_bucket_key IN LISTS _bucket_keys)
    get_property(_bucket GLOBAL PROPERTY
        "AROS_STAGED_HEADER_BINDING_INDEX_${_bucket_key}")
    list(LENGTH _bucket _binding_count)
    if(NOT _binding_count EQUAL 1)
        message(FATAL_ERROR
            "repeated preparation duplicated an index entry: ${_bucket}")
    endif()
endforeach()
string(SHA256 _changing_key "Dynamic/changing.h")
get_property(_changing_bucket GLOBAL PROPERTY
    "AROS_STAGED_HEADER_BINDING_INDEX_${_changing_key}")
list(LENGTH _changing_bucket _changing_binding_count)
if(NOT _changing_binding_count EQUAL 1 OR
   NOT _changing_bucket STREQUAL
       "Dynamic/changing.h|replacement-owner|replacement-hash|${_dynamic_changing}")
    message(FATAL_ERROR
        "replacement bucket is duplicated or stale: ${_changing_bucket}")
endif()
string(SHA256 _old_static_key "GL/gla.h")
get_property(_old_static_bucket GLOBAL PROPERTY
    "AROS_STAGED_HEADER_BINDING_INDEX_${_old_static_key}")
if(_old_static_bucket)
    message(FATAL_ERROR "old index bucket survived replacement: ${_old_static_bucket}")
endif()

_aros_collect_transitive_header_bindings(
    _dynamic_repeated_owners _dynamic_repeated_hashes "${_dynamic_root}")
if(NOT _dynamic_repeated_owners STREQUAL _dynamic_replaced_owners OR
   NOT _dynamic_repeated_hashes STREQUAL _dynamic_replaced_hashes)
    message(FATAL_ERROR
        "repeated collection changed owners/hashes: ${_dynamic_repeated_owners} / ${_dynamic_repeated_hashes}")
endif()
file(REMOVE_RECURSE "${_binding_temp_root}")

message(STATUS "transitive staged-header binding test passed")
