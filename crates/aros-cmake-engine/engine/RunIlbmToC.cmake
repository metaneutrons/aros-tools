cmake_minimum_required(VERSION 3.22)

# Execute ilbmtoc without shell redirection and publish the output atomically.

foreach(_required TOOL INPUT OUTPUT BINARY_ROOT)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "RunIlbmToC: ${_required} is required")
    endif()
endforeach()
if(NOT EXISTS "${TOOL}" OR IS_DIRECTORY "${TOOL}")
    message(FATAL_ERROR "RunIlbmToC: host tool does not exist: ${TOOL}")
endif()
if(NOT EXISTS "${INPUT}" OR IS_DIRECTORY "${INPUT}")
    message(FATAL_ERROR "RunIlbmToC: input does not exist: ${INPUT}")
endif()

cmake_path(ABSOLUTE_PATH BINARY_ROOT NORMALIZE OUTPUT_VARIABLE _binary_root)
cmake_path(ABSOLUTE_PATH OUTPUT NORMALIZE OUTPUT_VARIABLE _output)
cmake_path(IS_PREFIX _binary_root "${_output}" NORMALIZE _contained)
if(NOT _contained OR _output STREQUAL _binary_root)
    message(FATAL_ERROR "RunIlbmToC: output escapes the build tree: ${OUTPUT}")
endif()

set(_temporary "${_output}.tmp")
file(REMOVE "${_temporary}")
execute_process(
    COMMAND "${TOOL}" "${INPUT}"
    RESULT_VARIABLE _result
    OUTPUT_FILE "${_temporary}"
    ERROR_VARIABLE _error
    TIMEOUT 60)
if(NOT _result EQUAL 0)
    file(REMOVE "${_temporary}")
    string(STRIP "${_error}" _error)
    message(FATAL_ERROR
        "RunIlbmToC: ${TOOL} failed for ${INPUT} (exit ${_result}): ${_error}")
endif()
if(NOT EXISTS "${_temporary}" OR IS_DIRECTORY "${_temporary}")
    message(FATAL_ERROR "RunIlbmToC: generator produced no output for ${INPUT}")
endif()
file(RENAME "${_temporary}" "${_output}" RESULT _rename_result)
if(_rename_result)
    file(REMOVE "${_temporary}")
    message(FATAL_ERROR
        "RunIlbmToC: could not publish ${_output}: ${_rename_result}")
endif()
