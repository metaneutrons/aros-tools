# Bind one source module declaration to its native GNU KOBJ intermediate.
# All lists and linker values come from declaration-scoped source metadata.
# Missing configuration proof is an error; this bridge supplies no board or
# implicit Make defaults and never imports an existing Make-built payload.
include_guard(GLOBAL)
include("${CMAKE_CURRENT_LIST_DIR}/ModuleSourceGroups.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/ModuleKobjInputs.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/NativeKobj.cmake")

function(aros_add_source_native_kobj)
    cmake_parse_arguments(PARSE_ARGV 0 SK ""
        "NAME;OWNER;OUTPUT;LINKER;COLLECTOR;NM;OBJCOPY;LIBDIR" "DEPENDS")
    if(SK_UNPARSED_ARGUMENTS OR SK_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "Source native KOBJ received malformed arguments")
    endif()
    foreach(_required IN ITEMS NAME OWNER OUTPUT LINKER COLLECTOR NM OBJCOPY LIBDIR)
        if(NOT SK_${_required})
            message(FATAL_ERROR "Source native KOBJ requires ${_required}")
        endif()
    endforeach()
    if(NOT AROS_NATIVE_BUILD_CONTRACT)
        message(FATAL_ERROR "Source native KOBJ requires the verified native build contract")
    endif()
    get_property(_macro GLOBAL PROPERTY "AROS_MODULE_MACRO_${SK_OWNER}")
    if(_macro MATCHES "^(full|runtime-only)$")
        set(_form full)
    elseif(_macro STREQUAL "simple")
        set(_form simple)
    else()
        message(FATAL_ERROR "Source native KOBJ ${SK_OWNER}: unsupported source macro '${_macro}'")
    endif()

    # Resolve every consumed value before creating compilation/link targets.
    # `simple` does not read USER_OBJS, DEFNAME_LIBS or FUNCINSTR_LIBS in the
    # classic KOBJ rule, so uncertainty in those unused fields is harmless.
    aros_get_module_kobj_words(_uselibs "${SK_OWNER}" use_libs)
    aros_get_module_kobj_words(_kobj_flags "${SK_OWNER}" kobj_ldflags)
    aros_get_module_kobj_words(_user_flags "${SK_OWNER}" user_ldflags)
    aros_get_module_kobj_words(_script "${SK_OWNER}" kernel_kobj_ldscript)
    aros_get_module_kobj_words(_instrumentation "${SK_OWNER}" function_instrumentation)
    if("${_instrumentation}" STREQUAL "yes")
        # Classic Make also binds FUNCINSTR_FLAGS and the generated-start
        # exclusion list. Until those compile contracts are modeled, adding
        # only instrfunc to the link would falsely claim instrumented parity.
        message(FATAL_ERROR
            "Source native KOBJ ${SK_OWNER}: function instrumentation compile contract is not yet bound")
    elseif(NOT "${_instrumentation}" STREQUAL "no")
        message(FATAL_ERROR
            "Source native KOBJ ${SK_OWNER}: function instrumentation must be exactly yes or no")
    endif()
    set(_user_objects "")
    set(_defname_libs "")
    if(_form STREQUAL "full")
        aros_get_module_kobj_words(_user_objects "${SK_OWNER}" user_objects)
        aros_get_module_kobj_words(_defname_libs "${SK_OWNER}" defname_libs)
    endif()
    aros_create_kobj_compile_groups(_groups "${SK_OWNER}")
    # Relative USER_OBJS or linker-script expressions require an explicit
    # source/build-directory binding. NativeKobj refuses such unresolved
    # inputs rather than inferring a directory from the selected board.
    set(_args "")
    foreach(_group IN ITEMS START ARCH ASM C CXX END)
        if(_groups_${_group})
            list(APPEND _args "${_group}_OBJECTS" "${_groups_${_group}}")
        endif()
    endforeach()
    foreach(_pair IN ITEMS "USER_OBJECTS|_user_objects" "USELIBS|_uselibs"
            "DEFNAME_LIBS|_defname_libs" "KOBJ_FLAGS|_kobj_flags"
            "USER_FLAGS|_user_flags" "LDSCRIPT|_script")
        string(REPLACE "|" ";" _parts "${_pair}")
        list(GET _parts 0 _keyword)
        list(GET _parts 1 _variable)
        if(NOT "${${_variable}}" STREQUAL "")
            list(APPEND _args "${_keyword}" ${${_variable}})
        endif()
    endforeach()
    aros_add_native_kobj(NAME "${SK_NAME}" OWNER "${SK_OWNER}" FORM "${_form}"
        OUTPUT "${SK_OUTPUT}" LINKER "${SK_LINKER}" COLLECTOR "${SK_COLLECTOR}"
        NM "${SK_NM}" OBJCOPY "${SK_OBJCOPY}" LIBDIR "${SK_LIBDIR}"
        ${_args} DEPENDS ${_groups_TARGETS} ${SK_DEPENDS})
endfunction()
