cmake_minimum_required(VERSION 3.22)

foreach(_required IN ITEMS AROS_LIBDEFS_AUDIT_MANIFEST AROS_LIBDEFS_AUDIT_REPORT)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "${_required} is required")
    endif()
endforeach()
if(NOT EXISTS "${AROS_LIBDEFS_AUDIT_MANIFEST}")
    message(FATAL_ERROR
        "libdefs audit manifest does not exist: ${AROS_LIBDEFS_AUDIT_MANIFEST}")
endif()

function(_aros_read_functions_count path out_var)
    set(${out_var} "" PARENT_SCOPE)
    if(NOT EXISTS "${path}" OR IS_DIRECTORY "${path}")
        return()
    endif()
    file(STRINGS "${path}" _line
        REGEX "^#define[ \t]+FUNCTIONS_COUNT[ \t]+[0-9]+" LIMIT_COUNT 1)
    if(_line)
        string(REGEX REPLACE
            "^#define[ \t]+FUNCTIONS_COUNT[ \t]+([0-9]+).*$" "\\1"
            _value "${_line}")
        set(${out_var} "${_value}" PARENT_SCOPE)
    endif()
endfunction()

file(STRINGS "${AROS_LIBDEFS_AUDIT_MANIFEST}" _records)
set(_details "")
set(_compared 0)
set(_missing 0)
set(_under 0)
set(_over 0)
foreach(_record IN LISTS _records)
    string(REPLACE "\t" ";" _fields "${_record}")
    list(LENGTH _fields _field_count)
    if(NOT _field_count EQUAL 3)
        message(FATAL_ERROR "invalid libdefs audit record: ${_record}")
    endif()
    list(GET _fields 0 _identity)
    list(GET _fields 1 _rust_path)
    list(GET _fields 2 _reference_path)
    _aros_read_functions_count("${_rust_path}" _rust_count)
    _aros_read_functions_count("${_reference_path}" _reference_count)
    if("${_rust_count}" STREQUAL "" OR "${_reference_count}" STREQUAL "")
        math(EXPR _missing "${_missing} + 1")
        string(APPEND _details
            "missing\t${_identity}\trust=${_rust_count}\treference=${_reference_count}\n")
        continue()
    endif()
    math(EXPR _compared "${_compared} + 1")
    if(_rust_count LESS _reference_count)
        math(EXPR _under "${_under} + 1")
        string(APPEND _details
            "under\t${_identity}\trust=${_rust_count}\treference=${_reference_count}\n")
    elseif(_rust_count GREATER _reference_count)
        math(EXPR _over "${_over} + 1")
        string(APPEND _details
            "over\t${_identity}\trust=${_rust_count}\treference=${_reference_count}\n")
    endif()
endforeach()

math(EXPR _mismatches "${_under} + ${_over}")
string(CONCAT _report
    "aros-functions-count-audit-v1\n"
    "compared=${_compared}\n"
    "missing=${_missing}\n"
    "under=${_under}\n"
    "over=${_over}\n"
    "mismatches=${_mismatches}\n")
if(_details)
    string(APPEND _report "${_details}")
endif()
get_filename_component(_report_dir "${AROS_LIBDEFS_AUDIT_REPORT}" DIRECTORY)
file(MAKE_DIRECTORY "${_report_dir}")
set(_temporary "${AROS_LIBDEFS_AUDIT_REPORT}.tmp")
file(WRITE "${_temporary}" "${_report}")
execute_process(COMMAND "${CMAKE_COMMAND}" -E copy_if_different
    "${_temporary}" "${AROS_LIBDEFS_AUDIT_REPORT}"
    RESULT_VARIABLE _copy_result)
file(REMOVE "${_temporary}")
if(NOT _copy_result EQUAL 0)
    message(FATAL_ERROR "failed to publish ${AROS_LIBDEFS_AUDIT_REPORT}")
endif()

if(_missing GREATER 0 OR _mismatches GREATER 0)
    message(FATAL_ERROR
        "FUNCTIONS_COUNT audit failed: ${_missing} missing, ${_under} under, "
        "${_over} over; see ${AROS_LIBDEFS_AUDIT_REPORT}")
endif()
message(STATUS
    "FUNCTIONS_COUNT audit passed for ${_compared} shadow-capable header pair(s) -> "
    "${AROS_LIBDEFS_AUDIT_REPORT}")
