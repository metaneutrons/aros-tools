# Source-derived KOBJ compilation groups. This module retains source groups;
# it does not claim that the module's flags, extra objects, archives or final
# core/package link have been qualified merely because groups are available.
include_guard(GLOBAL)
include(CMakeParseArguments)

function(aros_resolve_kobj_source_groups out_prefix)
    cmake_parse_arguments(PARSE_ARGV 1 KG "" "OWNER;DIRECTORY"
        "SOURCES;CXX_SOURCES;OBJC_SOURCES;ASM_SOURCES;ARCH_SOURCES")
    if(KG_UNPARSED_ARGUMENTS OR KG_KEYWORDS_MISSING_VALUES OR
       NOT "${KG_OWNER}" MATCHES "^[A-Za-z0-9_.+-]+$" OR
       NOT IS_DIRECTORY "${KG_DIRECTORY}")
        message(FATAL_ERROR "Invalid KOBJ source-group declaration")
    endif()
    get_property(_form GLOBAL PROPERTY "AROS_MODULE_MACRO_${KG_OWNER}")
    if(NOT _form MATCHES "^(full|runtime-only|simple)$")
        message(FATAL_ERROR "KOBJ ${KG_OWNER}: missing or non-runtime source macro form")
    endif()
    foreach(_entry IN LISTS KG_ARCH_SOURCES)
        if(NOT "${_entry}" MATCHES "^[^|]+[|][^|]+[|][^|]+$")
            message(FATAL_ERROR "KOBJ ${KG_OWNER}: malformed architecture declaration")
        endif()
    endforeach()
    set(_kobj_cxx "${KG_CXX_SOURCES}")
    set(_kobj_asm "${KG_ASM_SOURCES}")
    if(_form STREQUAL "simple")
        set(_kobj_cxx "")
        set(_kobj_asm "")
    endif()
    aros_resolve_source_lanes(_flat "${KG_DIRECTORY}"
        MMAKE_ID "${KG_OWNER}" OUT_C _c OUT_CXX _cxx OUT_ASM _asm
        OUT_GAPS _source_gaps
        SOURCES ${KG_SOURCES} CXX_SOURCES ${_kobj_cxx}
        ASM_SOURCES ${_kobj_asm})
    list(FILTER _source_gaps EXCLUDE REGEX "^$")
    if(_source_gaps)
        message(FATAL_ERROR "KOBJ ${KG_OWNER}: unresolved declared source: ${_source_gaps}")
    endif()
    # OBJCFILES are not KOBJ prerequisites in either source macro. Retain
    # their declaration even when the runtime's language is disabled; that
    # runtime selection must not turn an excluded lane into a KOBJ gap.
    aros_resolve_sources(_objc "${KG_DIRECTORY}" LANGUAGE OBJC
        MMAKE_ID "${KG_OWNER}" SOURCES ${KG_OBJC_SOURCES})
    aros_resolve_arch_sources(_unused _dropped "${KG_DIRECTORY}"
        OUT_ARCH _arch OUT_GAPS _gaps
        SOURCES ${_flat} ARCH_SOURCES ${KG_ARCH_SOURCES})
    if(_gaps)
        message(FATAL_ERROR "KOBJ ${KG_OWNER}: unresolved architecture input: ${_gaps}")
    endif()

    # Make's wildcard is over <objdir>/arch/<basename>.o, not over absolute
    # source paths. Preserve that exact key; sorting source directories gives
    # a different order when the selected architecture lanes live apart.
    set(_arch_keys "")
    set(_seen_keys "")
    foreach(_source IN LISTS _arch)
        _aros_source_object_stem(_stem "${_source}")
        set(_key "${_stem}.o")
        if(_key IN_LIST _seen_keys OR "${_source}" MATCHES "[;|]")
            message(FATAL_ERROR "KOBJ ${KG_OWNER}: ambiguous architecture object ${_key}")
        endif()
        list(APPEND _seen_keys "${_key}")
        list(APPEND _arch_keys "${_key}|${_source}")
    endforeach()
    list(SORT _arch_keys COMPARE STRING CASE SENSITIVE ORDER ASCENDING)
    set(_ordered_arch "")
    foreach(_entry IN LISTS _arch_keys)
        string(REGEX REPLACE "^[^|]*[|]" "" _source "${_entry}")
        list(APPEND _ordered_arch "${_source}")
    endforeach()
    set(_nonarch_c "")
    foreach(_source IN LISTS _c)
        _aros_source_object_stem(_stem "${_source}")
        if(NOT "${_stem}.o" IN_LIST _seen_keys)
            list(APPEND _nonarch_c "${_source}")
        endif()
    endforeach()
    set(${out_prefix}_ARCH "${_ordered_arch}" PARENT_SCOPE)
    set(${out_prefix}_C "${_nonarch_c}" PARENT_SCOPE)
    # config/make.tmpl uses raw CXXFILES, not CXX_NARCHFILES, and does not
    # put OBJCFILES into the KOBJ. Keep OBJC explicitly separate, not dropped
    # into C. Runtime module compilation remains independently unchanged.
    set(${out_prefix}_OBJC_NON_KOBJ "${_objc}" PARENT_SCOPE)
    set(${out_prefix}_OBJC_DECLARED "${KG_OBJC_SOURCES}" PARENT_SCOPE)
    if(_form STREQUAL "simple")
        set(${out_prefix}_ASM "" PARENT_SCOPE)
        set(${out_prefix}_CXX "" PARENT_SCOPE)
    else()
        set(${out_prefix}_ASM "${_asm}" PARENT_SCOPE)
        set(${out_prefix}_CXX "${_cxx}" PARENT_SCOPE)
    endif()
endfunction()

# Compile each source-macro group independently with the finished module's
# target state. A single TARGET_OBJECTS expression cannot express the START /
# ARCH / ASM / C / CXX / END order. USER_OBJECTS are deliberately not invented
# here: they require the independently evaluated invocation-scope contract.
function(aros_create_kobj_compile_groups out_prefix owner)
    get_property(_known GLOBAL PROPERTY "AROS_KOBJ_SOURCE_GROUPS_${owner}")
    if(NOT _known OR NOT TARGET "${owner}")
        message(FATAL_ERROR "KOBJ ${owner}: no registered concrete source groups")
    endif()
    get_target_property(_type "${owner}" TYPE)
    get_target_property(_foreign "${owner}" AROS_FOREIGN_ARCH)
    get_target_property(_private "${owner}" AROS_PRIVATE_RUNTIME_ALTERNATIVE)
    if(NOT _type STREQUAL "EXECUTABLE" OR _foreign OR _private)
        message(FATAL_ERROR "KOBJ ${owner}: source owner is not an active runtime module")
    endif()
    set(_targets "")
    foreach(_group IN ITEMS START ARCH ASM C CXX END)
        get_property(_sources GLOBAL PROPERTY "AROS_KOBJ_SOURCES_${owner}_${_group}")
        set(${out_prefix}_${_group} "" PARENT_SCOPE)
        if(NOT _sources)
            continue()
        endif()
        string(TOLOWER "${_group}" _lower)
        set(_target "${owner}-kobj-${_lower}")
        if(TARGET "${_target}")
            get_target_property(_owner "${_target}" AROS_KOBJ_COMPILATION_OWNER)
            get_target_property(_actual_sources "${_target}" SOURCES)
            if(NOT "${_owner}" STREQUAL "${owner}" OR
               NOT "${_actual_sources}" STREQUAL "${_sources}")
                message(FATAL_ERROR "KOBJ ${owner}: compilation group ${_group} conflicts")
            endif()
        else()
            foreach(_source IN LISTS _sources)
                get_source_file_property(_external "${_source}" EXTERNAL_OBJECT)
                if(_external OR "${_source}" MATCHES "\\$<")
                    message(FATAL_ERROR "KOBJ ${owner}: ${_group} contains an unmodeled external object")
                endif()
            endforeach()
            add_library("${_target}" OBJECT EXCLUDE_FROM_ALL ${_sources})
            set_target_properties("${_target}" PROPERTIES
                AROS_KOBJ_COMPILATION_OWNER "${owner}")
        endif()
        foreach(_property IN ITEMS INCLUDE_DIRECTORIES SYSTEM_INCLUDE_DIRECTORIES
                COMPILE_DEFINITIONS COMPILE_OPTIONS COMPILE_FEATURES
                C_STANDARD C_STANDARD_REQUIRED C_EXTENSIONS
                CXX_STANDARD CXX_STANDARD_REQUIRED CXX_EXTENSIONS
                POSITION_INDEPENDENT_CODE AROS_NO_POSIXC_HEADERS
                AROS_QUOTE_FALLBACK_OPTIONS AROS_DEFER_QUOTE_OPTIONS
                LINK_LIBRARIES)
            get_target_property(_value "${owner}" "${_property}")
            if(NOT "${_value}" STREQUAL "_value-NOTFOUND")
                set_property(TARGET "${_target}" PROPERTY "${_property}" "${_value}")
            endif()
        endforeach()
        get_target_property(_deps "${owner}" MANUALLY_ADDED_DEPENDENCIES)
        if(_deps AND NOT "${_deps}" STREQUAL "_deps-NOTFOUND")
            add_dependencies("${_target}" ${_deps})
        endif()
        list(APPEND _targets "${_target}")
        set(${out_prefix}_${_group} "$<TARGET_OBJECTS:${_target}>" PARENT_SCOPE)
    endforeach()
    set(${out_prefix}_TARGETS "${_targets}" PARENT_SCOPE)
endfunction()

# Register after the concrete builder has generated declaration-owned START
# and END sources. All data comes from the same transpiled macro invocation.
function(aros_record_module_kobj_sources)
    cmake_parse_arguments(PARSE_ARGV 0 KS "" "OWNER;DIRECTORY"
        "SOURCES;CXX_SOURCES;OBJC_SOURCES;ASM_SOURCES;ARCH_SOURCES")
    if(KS_UNPARSED_ARGUMENTS OR KS_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "Malformed KOBJ source-group registration")
    endif()
    if(NOT AROS_NATIVE_BUILD_CONTRACT)
        return()
    endif()
    get_property(_known GLOBAL PROPERTY "AROS_KOBJ_SOURCE_GROUPS_${KS_OWNER}" SET)
    if(_known)
        message(FATAL_ERROR "Duplicate KOBJ source groups for ${KS_OWNER}")
    endif()
    aros_resolve_kobj_source_groups(_groups OWNER "${KS_OWNER}"
        DIRECTORY "${KS_DIRECTORY}" SOURCES ${KS_SOURCES}
        CXX_SOURCES ${KS_CXX_SOURCES} OBJC_SOURCES ${KS_OBJC_SOURCES}
        ASM_SOURCES ${KS_ASM_SOURCES} ARCH_SOURCES ${KS_ARCH_SOURCES})
    get_property(_form GLOBAL PROPERTY "AROS_MODULE_MACRO_${KS_OWNER}")
    foreach(_group IN ITEMS ARCH ASM C CXX OBJC_NON_KOBJ OBJC_DECLARED)
        set_property(GLOBAL PROPERTY "AROS_KOBJ_SOURCES_${KS_OWNER}_${_group}"
            "${_groups_${_group}}")
    endforeach()
    foreach(_group IN ITEMS START END)
        if(_form STREQUAL "simple")
            set(_sources "")
        else()
            get_property(_sources GLOBAL PROPERTY "AROS_MODULE_${_group}_${KS_OWNER}")
        endif()
        set_property(GLOBAL PROPERTY "AROS_KOBJ_SOURCES_${KS_OWNER}_${_group}" "${_sources}")
    endforeach()
    set_property(GLOBAL PROPERTY "AROS_KOBJ_SOURCE_GROUPS_${KS_OWNER}" TRUE)
endfunction()
