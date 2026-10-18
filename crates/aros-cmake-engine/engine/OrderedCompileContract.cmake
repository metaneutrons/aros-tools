include_guard(GLOBAL)

# Keep one reviewed ordered argument sequence intact through CMake's option
# de-duplication. In particular, -DVALUE=1 -UVALUE -DVALUE=1 must not become
# -DVALUE=1 -UVALUE. This helper does not authenticate or derive the contract;
# its caller must supply already evaluated, source-bound arguments.
function(_aros_target_ordered_compile_contract target language)
    if(NOT TARGET "${target}")
        message(FATAL_ERROR "Ordered compile contract requires an existing target")
    endif()
    if(NOT language STREQUAL "ALL" AND
       NOT language STREQUAL "C" AND NOT language STREQUAL "CXX")
        message(FATAL_ERROR "Ordered compile contract language must be ALL, C or CXX")
    endif()
    if(ARGC LESS 3)
        message(FATAL_ERROR "Ordered compile contract requires nonempty arguments")
    endif()
    get_property(_languages TARGET "${target}"
        PROPERTY AROS_ORDERED_COMPILE_CONTRACT_LANGUAGES)
    if(language IN_LIST _languages OR "ALL" IN_LIST _languages OR
       (language STREQUAL "ALL" AND NOT "${_languages}" STREQUAL ""))
        # CMake can also de-duplicate identical SHELL groups across calls.
        # Supply the complete sequence once per language, never in fragments.
        message(FATAL_ERROR "Ordered compile contracts must not overlap languages")
    endif()
    set(_group "")
    math(EXPR _last "${ARGC} - 1")
    foreach(_index RANGE 2 ${_last})
        # ARGVn preserves an individual argument containing a semicolon, unlike
        # iterating ARGN as a list. Reject it before any list expansion.
        set(_argument "${ARGV${_index}}")
        if(_argument STREQUAL "" OR _argument MATCHES "[;\r\n$<>]" OR
           _argument MATCHES "^@")
            message(FATAL_ERROR "Ordered compile contract contains an unsafe argument")
        endif()
        # UNIX_COMMAND parsing is used by CMake's SHELL option on all supported
        # hosts. Quote each argv item, including literal quotes and whitespace.
        string(REPLACE "'" "'\\''" _quoted "${_argument}")
        if(NOT _group STREQUAL "")
            string(APPEND _group " ")
        endif()
        string(APPEND _group "'${_quoted}'")
    endforeach()
    set_property(TARGET "${target}" APPEND PROPERTY
        AROS_ORDERED_COMPILE_CONTRACT_LANGUAGES "${language}")
    if(language STREQUAL "ALL")
        target_compile_options("${target}" PRIVATE "SHELL:${_group}")
    else()
        target_compile_options("${target}" PRIVATE
            "$<$<COMPILE_LANGUAGE:${language}>:SHELL:${_group}>")
    endif()
endfunction()
