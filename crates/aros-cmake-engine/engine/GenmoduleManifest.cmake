# Deterministic output manifests for the reference tools/genmodule writefiles
# command.
#
# CMake cannot add sources discovered by a build-time glob to a normal static
# library.  genmodule's names are not actually dynamic, though: every STACK
# function produces one source, REGISTER functions share one source, and the
# remaining support files follow fixed names.  Reading that manifest from the
# source .conf at configure time lets Ninja own every generated file without
# running the generator early or globbing an as-yet empty build directory.

include_guard(GLOBAL)
include(CMakeParseArguments)

# _aros_genmodule_function_is_stack(<out-var> <prototype>)
#
# A function without a second parenthesised register list is a STACK call in
# tools/genmodule/config.c.  Function-pointer arguments contain parentheses of
# their own, so find the close matching the prototype's first open parenthesis
# instead of using a regular expression over the whole line.
function(_aros_genmodule_function_is_stack out_var prototype)
    string(FIND "${prototype}" "(" _open)
    if(_open LESS 0)
        message(FATAL_ERROR
            "genmodule manifest: malformed function prototype '${prototype}'")
    endif()

    string(LENGTH "${prototype}" _length)
    set(_depth 0)
    set(_close -1)
    set(_index ${_open})
    while(_index LESS _length)
        string(SUBSTRING "${prototype}" ${_index} 1 _char)
        if("${_char}" STREQUAL "(")
            math(EXPR _depth "${_depth} + 1")
        elseif("${_char}" STREQUAL ")")
            math(EXPR _depth "${_depth} - 1")
            if(_depth EQUAL 0)
                set(_close ${_index})
                break()
            endif()
        endif()
        math(EXPR _index "${_index} + 1")
    endwhile()
    if(_close LESS 0)
        message(FATAL_ERROR
            "genmodule manifest: unmatched '(' in prototype '${prototype}'")
    endif()

    math(EXPR _tail_start "${_close} + 1")
    string(SUBSTRING "${prototype}" ${_tail_start} -1 _tail)
    string(STRIP "${_tail}" _tail)
    if(_tail MATCHES "^\\(")
        set(${out_var} FALSE PARENT_SCOPE)
    else()
        set(${out_var} TRUE PARENT_SCOPE)
    endif()
endfunction()

# aros_genmodule_writefiles_manifest(
#     <prefix>
#     CONFIG <file> MODULE <name> MODTYPE <type>
#     GEN_DIR <directory> STUB_DIR <directory>)
#
# Exports these lists to the caller:
#   <prefix>_NORMAL_STACK_STUBS / <prefix>_REL_STACK_STUBS
#   <prefix>_NORMAL_REGCALL_STUBS / <prefix>_REL_REGCALL_STUBS
#   <prefix>_NORMAL_STUBS / <prefix>_REL_STUBS (the union of both)
#   <prefix>_NORMAL_AUTOINIT / <prefix>_REL_AUTOINIT
#   <prefix>_NORMAL_GETLIBBASE / <prefix>_REL_GETLIBBASE
#   <prefix>_HAS_REL_LINKLIB / <prefix>_RELLIBS
#   <prefix>_RUNTIME_DEFINES / <prefix>_LINKLIB_DEFINES
#   <prefix>_ALL_OUTPUTS
#
# The parser intentionally covers the writefiles naming grammar, not the full
# .conf language.  The reference parser likewise requires each function
# declaration to occupy one line.  Unknown directives do not create files and
# are ignored; a malformed declaration fails configuration rather than
# silently yielding an incomplete archive.
function(aros_genmodule_writefiles_manifest prefix)
    set(oneValueArgs CONFIG MODULE MODTYPE GEN_DIR STUB_DIR)
    cmake_parse_arguments(GM "" "${oneValueArgs}" "" ${ARGN})

    foreach(_required CONFIG MODULE MODTYPE GEN_DIR STUB_DIR)
        if(NOT GM_${_required})
            message(FATAL_ERROR
                "aros_genmodule_writefiles_manifest: ${_required} is required")
        endif()
    endforeach()
    if(NOT EXISTS "${GM_CONFIG}")
        message(FATAL_ERROR
            "aros_genmodule_writefiles_manifest: missing ${GM_CONFIG}")
    endif()

    # The source list below is part of the generated build graph.  Make CMake
    # regenerate that graph when the declaration gains or loses functions;
    # otherwise Ninja would keep a stale set of OUTPUTs and archive members.
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
        "${GM_CONFIG}")

    file(STRINGS "${GM_CONFIG}" _lines)
    set(_section "")
    set(_nested 0)
    set(_stack_functions "")
    set(_last_function_stack FALSE)
    set(_has_functions FALSE)
    set(_stubs AUTO)
    set(_autoinit AUTO)
    set(_rellinklib FALSE)
    set(_rellibs "")

    foreach(_line IN LISTS _lines)
        string(STRIP "${_line}" _trimmed)
        if(_trimmed MATCHES "^##begin (interface|class)([ \\t]|$)")
            math(EXPR _nested "${_nested} + 1")
            continue()
        elseif(_trimmed MATCHES "^##end (interface|class)([ \\t]|$)")
            if(_nested GREATER 0)
                math(EXPR _nested "${_nested} - 1")
            endif()
            continue()
        elseif(_nested GREATER 0)
            continue()
        elseif("${_trimmed}" STREQUAL "##begin config")
            set(_section config)
            continue()
        elseif("${_trimmed}" STREQUAL "##end config")
            set(_section "")
            continue()
        elseif("${_trimmed}" STREQUAL "##begin functionlist")
            set(_section functionlist)
            continue()
        elseif("${_trimmed}" STREQUAL "##begin cfunctionlist")
            set(_section cfunctionlist)
            continue()
        elseif("${_trimmed}" STREQUAL "##end functionlist")
            set(_section "")
            continue()
        elseif("${_trimmed}" STREQUAL "##end cfunctionlist")
            set(_section "")
            continue()
        elseif(_trimmed MATCHES "^##begin ")
            set(_section other)
            continue()
        elseif(_trimmed MATCHES "^##end ")
            set(_section "")
            continue()
        endif()

        if("${_section}" STREQUAL "config" AND
           "${_trimmed}" MATCHES "^options[ \\t]+(.*)$")
            set(_options "${CMAKE_MATCH_1}")
            string(REPLACE "," ";" _options "${_options}")
            string(REPLACE " " ";" _options "${_options}")
            string(REPLACE "\t" ";" _options "${_options}")
            foreach(_option IN LISTS _options)
                if("${_option}" STREQUAL "stubs")
                    set(_stubs ON)
                elseif("${_option}" STREQUAL "nostubs")
                    set(_stubs OFF)
                elseif("${_option}" STREQUAL "autoinit")
                    set(_autoinit ON)
                elseif("${_option}" STREQUAL "noautoinit")
                    set(_autoinit OFF)
                elseif("${_option}" STREQUAL "rellinklib")
                    set(_rellinklib TRUE)
                endif()
            endforeach()
            continue()
        endif()
        if("${_section}" STREQUAL "config" AND
           "${_trimmed}" MATCHES "^rellib[ \\t]+")
            string(REGEX REPLACE "^rellib[ \\t]+" "" _rellib "${_trimmed}")
            string(REPLACE "\t" " " _rellib "${_rellib}")
            string(REGEX REPLACE "[ #].*$" "" _rellib "${_rellib}")
            list(APPEND _rellibs "${_rellib}")
            continue()
        endif()

        if((NOT "${_section}" STREQUAL "functionlist" AND
            NOT "${_section}" STREQUAL "cfunctionlist") OR
           "${_trimmed}" STREQUAL "" OR
           _trimmed MATCHES "^#")
            continue()
        endif()

        if(_trimmed MATCHES "^\\.cfunction([ \\t]|$)")
            # This directive changes the preceding no-register declaration
            # from STACK to REGISTER in config.c.
            if(_last_function_stack)
                list(POP_BACK _stack_functions)
                set(_last_function_stack FALSE)
            endif()
            continue()
        elseif(_trimmed MATCHES "^\\.")
            continue()
        endif()

        # Strip an optional trailing comment, then take the final identifier
        # before the prototype's first open parenthesis as the public name.
        string(FIND "${_trimmed}" "#" _comment)
        if(NOT _comment LESS 0)
            string(SUBSTRING "${_trimmed}" 0 ${_comment} _trimmed)
            string(STRIP "${_trimmed}" _trimmed)
        endif()
        string(FIND "${_trimmed}" "(" _open)
        if(_open LESS 0)
            message(FATAL_ERROR
                "${GM_CONFIG}: malformed function declaration '${_trimmed}'")
        endif()
        string(SUBSTRING "${_trimmed}" 0 ${_open} _declarator)
        string(STRIP "${_declarator}" _declarator)
        string(REGEX MATCH "[A-Za-z_][A-Za-z0-9_]*$" _function "${_declarator}")
        if(NOT _function)
            message(FATAL_ERROR
                "${GM_CONFIG}: cannot determine function name in '${_trimmed}'")
        endif()

        set(_has_functions TRUE)
        _aros_genmodule_function_is_stack(_is_stack "${_trimmed}")
        # `cfunctionlist` changes the convention used by declarations that
        # carry an explicit register list. A declaration without that second
        # list remains STACK, just as in an ordinary functionlist.
        if(_is_stack)
            list(APPEND _stack_functions "${_function}")
            set(_last_function_stack TRUE)
        else()
            set(_last_function_stack FALSE)
        endif()
    endforeach()

    if("${_stubs}" STREQUAL "AUTO")
        if("${GM_MODTYPE}" STREQUAL "library" AND _has_functions)
            set(_stubs ON)
        else()
            set(_stubs OFF)
        endif()
    endif()
    if("${_autoinit}" STREQUAL "AUTO")
        if("${GM_MODTYPE}" STREQUAL "library")
            set(_autoinit ON)
        else()
            set(_autoinit OFF)
        endif()
    endif()

    set(_normal_stack_stubs "")
    set(_rel_stack_stubs "")
    set(_normal_regcall_stubs "")
    set(_rel_regcall_stubs "")
    if(_stubs)
        foreach(_function IN LISTS _stack_functions)
            list(APPEND _normal_stack_stubs
                "${GM_STUB_DIR}/${GM_MODULE}_${_function}_stub.c")
            if(_rellinklib)
                list(APPEND _rel_stack_stubs
                    "${GM_STUB_DIR}/${GM_MODULE}_${_function}_relstub.c")
            endif()
        endforeach()
        # writestubs.c creates the aggregate even if every API uses STACK.
        list(APPEND _normal_regcall_stubs
            "${GM_STUB_DIR}/${GM_MODULE}_regcall_stubs.c")
        if(_rellinklib)
            list(APPEND _rel_regcall_stubs
                "${GM_STUB_DIR}/${GM_MODULE}_regcall_relstubs.c")
        endif()
    endif()

    # GNU Make's wildcard function sorts each matched pattern independently.
    # Preserve that ordering because it determines archive-member order in the
    # original build (while retaining the source expression's component order).
    list(SORT _normal_stack_stubs)
    list(SORT _rel_stack_stubs)
    list(SORT _normal_regcall_stubs)
    list(SORT _rel_regcall_stubs)
    set(_normal_stubs ${_normal_stack_stubs} ${_normal_regcall_stubs})
    set(_rel_stubs ${_rel_stack_stubs} ${_rel_regcall_stubs})

    set(_normal_autoinit "")
    set(_rel_autoinit "")
    if(_autoinit)
        set(_normal_autoinit
            "${GM_STUB_DIR}/${GM_MODULE}_autoinit.c")
        if(_rellinklib)
            set(_rel_autoinit
                "${GM_STUB_DIR}/${GM_MODULE}_relautoinit.c")
        endif()
    endif()

    set(_normal_getlibbase "")
    set(_rel_getlibbase "")
    if("${GM_MODTYPE}" STREQUAL "library")
        set(_normal_getlibbase
            "${GM_STUB_DIR}/${GM_MODULE}_getlibbase.c")
        if(_rellinklib)
            set(_rel_getlibbase
                "${GM_STUB_DIR}/${GM_MODULE}_relgetlibbase.c")
        endif()
    endif()

    set(_all_outputs
        "${GM_GEN_DIR}/${GM_MODULE}_start.c"
        "${GM_GEN_DIR}/${GM_MODULE}_end.c"
        "${GM_GEN_DIR}/${GM_MODULE}${GM_MODTYPE}.entrypoints"
        ${_normal_stubs} ${_rel_stubs}
        ${_normal_autoinit} ${_rel_autoinit}
        ${_normal_getlibbase} ${_rel_getlibbase})

    # Match tools/genmodule/writemakefile.c: relative-library base selection
    # applies to both the runtime implementation and the generated client
    # archives. A module with its own relative link library additionally
    # suppresses the runtime's ordinary global base.
    set(_runtime_defines "")
    set(_linklib_defines "")
    foreach(_rellib IN LISTS _rellibs)
        string(TOUPPER "${_rellib}" _rellib_upper)
        list(APPEND _runtime_defines "__${_rellib_upper}_RELLIBBASE__")
        list(APPEND _linklib_defines "__${_rellib_upper}_RELLIBBASE__")
    endforeach()
    if(_rellinklib)
        string(TOUPPER "${GM_MODULE}" _module_upper)
        list(APPEND _runtime_defines "__${_module_upper}_NOLIBBASE__")
    endif()

    set(${prefix}_NORMAL_STACK_STUBS "${_normal_stack_stubs}" PARENT_SCOPE)
    set(${prefix}_REL_STACK_STUBS "${_rel_stack_stubs}" PARENT_SCOPE)
    set(${prefix}_NORMAL_REGCALL_STUBS "${_normal_regcall_stubs}" PARENT_SCOPE)
    set(${prefix}_REL_REGCALL_STUBS "${_rel_regcall_stubs}" PARENT_SCOPE)
    set(${prefix}_NORMAL_STUBS "${_normal_stubs}" PARENT_SCOPE)
    set(${prefix}_REL_STUBS "${_rel_stubs}" PARENT_SCOPE)
    set(${prefix}_NORMAL_AUTOINIT "${_normal_autoinit}" PARENT_SCOPE)
    set(${prefix}_REL_AUTOINIT "${_rel_autoinit}" PARENT_SCOPE)
    set(${prefix}_NORMAL_GETLIBBASE "${_normal_getlibbase}" PARENT_SCOPE)
    set(${prefix}_REL_GETLIBBASE "${_rel_getlibbase}" PARENT_SCOPE)
    set(${prefix}_HAS_REL_LINKLIB "${_rellinklib}" PARENT_SCOPE)
    set(${prefix}_RELLIBS "${_rellibs}" PARENT_SCOPE)
    set(${prefix}_RUNTIME_DEFINES "${_runtime_defines}" PARENT_SCOPE)
    set(${prefix}_LINKLIB_DEFINES "${_linklib_defines}" PARENT_SCOPE)
    set(${prefix}_ALL_OUTPUTS "${_all_outputs}" PARENT_SCOPE)
endfunction()
