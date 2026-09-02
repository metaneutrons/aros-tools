cmake_minimum_required(VERSION 3.22)

# Run one FlexCat catalog conversion while preserving the legacy warning
# contract. FlexCat returns values below 10 for non-fatal warnings; MetaMake's
# `%build_catalogs` accepts those and fails only on 10 or above.

foreach(_required TOOL OUTPUT)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "RunFlexCat.cmake: ${_required} is required")
    endif()
endforeach()

set(_arguments "")
if(DEFINED POFILE AND NOT "${POFILE}" STREQUAL "")
    # The hand-written MUI rules use FlexCat's POFILE mode directly. It has
    # different positional semantics from translated `.ct` catalog mode.
    list(APPEND _arguments "POFILE" "${POFILE}" "CATALOG=${OUTPUT}")
elseif(DEFINED SOURCE_DESCRIPTION AND NOT "${SOURCE_DESCRIPTION}" STREQUAL "")
    if(NOT DEFINED DESCRIPTION OR "${DESCRIPTION}" STREQUAL "")
        message(FATAL_ERROR "RunFlexCat.cmake: DESCRIPTION is required for a source output")
    endif()
    list(APPEND _arguments "${DESCRIPTION}" "${OUTPUT}=${SOURCE_DESCRIPTION}")
else()
    foreach(_required DESCRIPTION TRANSLATION)
        if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
            message(FATAL_ERROR "RunFlexCat.cmake: ${_required} is required for a catalog output")
        endif()
    endforeach()
    if(DEFINED CONVERSION AND NOT "${CONVERSION}" STREQUAL "")
        list(APPEND _arguments "${CONVERSION}")
    endif()
    list(APPEND _arguments
        "${DESCRIPTION}"
        "${TRANSLATION}"
        "CATALOG=${OUTPUT}")
endif()

execute_process(
    COMMAND "${TOOL}" ${_arguments}
    RESULT_VARIABLE _result)

if(NOT "${_result}" MATCHES "^[0-9]+$")
    message(FATAL_ERROR
        "FlexCat could not create ${OUTPUT}: ${_result}")
endif()
if(_result GREATER_EQUAL 10)
    message(FATAL_ERROR
        "FlexCat failed creating ${OUTPUT} with status ${_result}")
endif()
