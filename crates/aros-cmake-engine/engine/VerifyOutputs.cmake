cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED MANIFEST OR MANIFEST STREQUAL "")
    message(FATAL_ERROR "VerifyOutputs.cmake requires MANIFEST")
endif()
if(NOT EXISTS "${MANIFEST}" OR IS_DIRECTORY "${MANIFEST}")
    message(FATAL_ERROR "External product manifest does not exist: ${MANIFEST}")
endif()

include("${MANIFEST}")
if(NOT DEFINED EXPECTED_OUTPUTS OR NOT EXPECTED_OUTPUTS)
    message(FATAL_ERROR
        "External product manifest contains no EXPECTED_OUTPUTS: ${MANIFEST}")
endif()

set(_missing "")
foreach(_output IN LISTS EXPECTED_OUTPUTS)
    if(NOT EXISTS "${_output}" OR IS_DIRECTORY "${_output}")
        list(APPEND _missing "${_output}")
    endif()
endforeach()
if(_missing)
    string(JOIN "\n  " _missing_report ${_missing})
    message(FATAL_ERROR
        "External CMake install did not produce its declared output(s):\n"
        "  ${_missing_report}")
endif()
