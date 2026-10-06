# Exact source invocation metadata. This does not infer a KOBJ contract from a
# runtime suffix, a config file or a board ID: those are different source facts.
include_guard(GLOBAL)
include(CMakeParseArguments)

function(aros_record_module_macro)
    cmake_parse_arguments(PARSE_ARGV 0 MM "" "OWNER;FORM" "")
    if(MM_UNPARSED_ARGUMENTS OR MM_KEYWORDS_MISSING_VALUES OR
       NOT "${MM_OWNER}" MATCHES "^[A-Za-z0-9_.+-]+$" OR
       NOT "${MM_FORM}" MATCHES "^(full|runtime-only|abi-only|simple)$")
        message(FATAL_ERROR "Invalid source module macro contract: owner='${MM_OWNER}', form='${MM_FORM}'")
    endif()
    get_property(_recorded GLOBAL PROPERTY "AROS_MODULE_MACRO_${MM_OWNER}" SET)
    if(_recorded)
        message(FATAL_ERROR "Duplicate source module macro contract for ${MM_OWNER}")
    endif()
    set_property(GLOBAL PROPERTY "AROS_MODULE_MACRO_${MM_OWNER}" "${MM_FORM}")
endfunction()
